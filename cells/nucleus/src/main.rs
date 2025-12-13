// cells/nucleus/src/main.rs
// SPDX-License-Identifier: MIT
// The Nucleus: System-wide singleton that manages Cell infrastructure

use cell_sdk::*;
use anyhow::{Result, anyhow};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use cell_discovery::Discovery;
use cell_model::manifest::{MeshManifest, PlacementStrategy, ResourceLimits};

// Define explicit remote to Mesh so we can query the graph
cell_remote!(Mesh = "mesh");

// === PROTOCOL DEFINITIONS ===

#[protein]
pub struct NucleusStatus {
    pub uptime_secs: u64,
    pub managed_cells: Vec<String>,
    pub system_health: HealthMetrics,
}

#[protein]
pub struct HealthMetrics {
    pub cpu_usage: f64,
    pub memory_mb: u64,
    pub active_connections: u64,
}

#[protein]
pub struct CellRegistration {
    pub name: String,
    pub node_id: u64,
    pub capabilities: Vec<String>,
    pub endpoints: Vec<String>,
}

#[protein]
pub struct DiscoveryQuery {
    pub cell_name: String,
    pub prefer_local: bool,
}

#[protein]
pub struct DiscoveryResult {
    pub instances: Vec<CellInstance>,
}

#[protein]
pub struct CellInstance {
    pub node_id: u64,
    pub address: String,
    pub latency_us: u64,
    pub health_score: f64,
}

#[protein]
pub struct ApplyManifest {
    pub yaml: String,
}

#[protein]
pub struct ScheduleSpore {
    pub spore_id: String,
    pub required_caps: String,
}

#[protein]
pub struct PruneResult {
    pub killed: Vec<String>,
}

// === NUCLEUS SERVICE ===

pub struct Nucleus {
    start_time: std::time::SystemTime,
    registry: Arc<RwLock<CellRegistry>>,
    state: Arc<RwLock<NucleusState>>,
}

struct NucleusState {
    desired_state: Option<MeshManifest>,
    spores: HashMap<String, Vec<u8>>,
}

struct CellRegistry {
    cells: HashMap<String, Vec<CellRegistration>>,
    last_heartbeat: HashMap<String, std::time::Instant>,
}

impl Nucleus {
    pub fn new() -> Self {
        Self {
            start_time: std::time::SystemTime::now(),
            registry: Arc::new(RwLock::new(CellRegistry {
                cells: HashMap::new(),
                last_heartbeat: HashMap::new(),
            })),
            state: Arc::new(RwLock::new(NucleusState {
                desired_state: None,
                spores: HashMap::new(),
            })),
        }
    }

    pub async fn start_background_tasks(&self) {
        let registry = self.registry.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let mut reg = registry.write().await;
                let now = std::time::Instant::now();
                
                // Prune heartbeats older than 30s
                reg.last_heartbeat.retain(|_, last| {
                    now.duration_since(*last).as_secs() < 30
                });
                
                // Sync registry with heartbeats
                let CellRegistry { cells, last_heartbeat } = &mut *reg;
                let mut empty_keys = Vec::new();
                for (name, instances) in cells.iter_mut() {
                    if !last_heartbeat.contains_key(name) {
                        instances.clear();
                    }
                    if instances.is_empty() {
                        empty_keys.push(name.clone());
                    }
                }
                for k in empty_keys {
                    cells.remove(&k);
                }
            }
        });
    }

    // --- SERVICE IMPLEMENTATION ---

    pub async fn register(&self, reg: CellRegistration) -> Result<bool> {
        let mut registry = self.registry.write().await;
        let instances = registry.cells.entry(reg.name.clone()).or_insert_with(Vec::new);
        instances.retain(|r| r.node_id != reg.node_id);
        instances.push(reg.clone());
        registry.last_heartbeat.insert(reg.name.clone(), std::time::Instant::now());
        tracing::info!("[Nucleus] Registered cell '{}' (Node {})", reg.name, reg.node_id);
        Ok(true)
    }

    pub async fn discover(&self, query: DiscoveryQuery) -> Result<DiscoveryResult> {
        let registry = self.registry.read().await;
        let mut instances = Vec::new();
        if let Some(regs) = registry.cells.get(&query.cell_name) {
            for reg in regs {
                let address = reg.endpoints.first().cloned().unwrap_or_default();
                instances.push(CellInstance {
                    node_id: reg.node_id,
                    address,
                    latency_us: 0,
                    health_score: 1.0,
                });
            }
        }
        Ok(DiscoveryResult { instances })
    }

    pub async fn status(&self) -> Result<NucleusStatus> {
        let registry = self.registry.read().await;
        let managed_cells = registry.cells.keys().cloned().collect();
        let uptime = std::time::SystemTime::now()
            .duration_since(self.start_time)
            .unwrap_or_default()
            .as_secs();
        Ok(NucleusStatus {
            uptime_secs: uptime,
            managed_cells,
            system_health: HealthMetrics { cpu_usage: 0.0, memory_mb: 0, active_connections: 0 },
        })
    }

    pub async fn heartbeat(&self, cell_name: String) -> Result<bool> {
        let mut registry = self.registry.write().await;
        if registry.cells.contains_key(&cell_name) {
            registry.last_heartbeat.insert(cell_name, std::time::Instant::now());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // --- GARBAGE COLLECTION ---
    
    pub async fn prune(&self) -> Result<PruneResult> {
        tracing::info!("[Nucleus] Starting Mesh Garbage Collection...");
        
        let mut killed_total = Vec::new();
        
        // 1. Get Dependency Graph from Mesh Cell
        let mut mesh_client = Mesh::Client::connect().await
            .context("Cannot connect to Mesh to analyze dependencies")?;
            
        // Map: Consumer -> Vec<Providers>
        let graph_raw = mesh_client.get_graph().await
            .context("Failed to retrieve graph")?;

        let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
        for (k, v) in graph_raw {
            graph.insert(k, v.into_iter().collect());
        }

        // Loop until convergence
        loop {
            // Get currently active cells from registry
            let active_cells: HashSet<String> = {
                let reg = self.registry.read().await;
                reg.cells.keys().cloned().collect()
            };

            if active_cells.is_empty() { break; }

            // Protected System Cells
            let protected: HashSet<&str> = ["nucleus", "mesh", "axon", "hypervisor", "mycelium", "builder", "observer", "ca", "vault", "iam"].into();

            // Find cells that have NO active consumers
            let mut active_consumers_count: HashMap<String, u32> = HashMap::new();
            
            // Initialize counts
            for cell in &active_cells {
                active_consumers_count.insert(cell.clone(), 0);
            }

            // Populate counts based on graph + active list
            for (consumer, providers) in &graph {
                if active_cells.contains(consumer) {
                    for provider in providers {
                        if active_cells.contains(provider) {
                            *active_consumers_count.entry(provider.clone()).or_default() += 1;
                        }
                    }
                }
            }

            let mut iteration_kills = Vec::new();

            for cell in active_cells {
                if protected.contains(cell.as_str()) {
                    continue; 
                }

                let count = active_consumers_count.get(&cell).unwrap_or(&0);
                if *count == 0 {
                    iteration_kills.push(cell);
                }
            }

            if iteration_kills.is_empty() {
                break; // Converged
            }

            // Kill them
            for target in &iteration_kills {
                tracing::info!("[Nucleus] Pruning unused cell: {}", target);
                
                // Connect and send Shutdown Ops command
                // We use Synapse manually to access the OPS channel
                if let Ok(mut synapse) = Synapse::grow(target).await {
                    let req = cell_model::ops::OpsRequest::Shutdown;
                    let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
                    
                    // Fire and forget (or wait for ack)
                    let _ = synapse.fire_on_channel(cell_core::channel::OPS, &req_bytes).await;
                }

                // Remove from local registry immediately so next loop iteration sees it as gone
                let mut reg = self.registry.write().await;
                reg.cells.remove(target);
                reg.last_heartbeat.remove(target);
                
                killed_total.push(target.clone());
            }
            
            // Short sleep to allow network propagation before next graph analysis?
            // Actually we simulated the removal in our local set, so we can recurse immediately.
        }

        Ok(PruneResult { killed: killed_total })
    }
}

#[service]
#[derive(Clone)]
struct NucleusService {
    inner: Arc<Nucleus>
}

#[handler]
impl NucleusService {
    async fn register(&self, reg: CellRegistration) -> Result<bool> {
        self.inner.register(reg).await
    }

    async fn discover(&self, query: DiscoveryQuery) -> Result<DiscoveryResult> {
        self.inner.discover(query).await
    }

    async fn status(&self) -> Result<NucleusStatus> {
        self.inner.status().await
    }

    async fn heartbeat(&self, cell_name: String) -> Result<bool> {
        self.inner.heartbeat(cell_name).await
    }

    async fn apply(&self, req: ApplyManifest) -> Result<bool> {
        let manifest: MeshManifest = serde_yaml::from_str(&req.yaml)
            .map_err(|e| anyhow!("Invalid YAML: {}", e))?;
        let mut state = self.inner.state.write().await;
        state.desired_state = Some(manifest.clone());
        tracing::info!("[Nucleus] Applied manifest for mesh '{}'", manifest.mesh);
        Ok(true)
    }

    async fn schedule(&self, req: ScheduleSpore) -> Result<String> {
        tracing::info!("[Nucleus] Scheduling spore '{}'...", req.spore_id);
        Ok("127.0.0.1:9000".to_string())
    }

    // The GC method
    async fn vacuum(&self) -> Result<PruneResult> {
        self.inner.prune().await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    let nucleus = Nucleus::new();
    nucleus.start_background_tasks().await;
    
    println!("[Nucleus] System manager active");
    
    let service = NucleusService { inner: Arc::new(nucleus) };
    service.serve("nucleus").await
}