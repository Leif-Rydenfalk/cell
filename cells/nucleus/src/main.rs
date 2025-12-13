// cells/nucleus/src/main.rs
// SPDX-License-Identifier: MIT
// The Nucleus: System-wide singleton that manages Cell infrastructure

use cell_sdk::*;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use cell_discovery::Discovery;
use cell_model::manifest::MeshManifest; 
use cell_model::manifest::{PlacementStrategy, ResourceLimits}; // Fixed imports

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

// === NUCLEUS SERVICE ===

pub struct Nucleus {
    start_time: std::time::SystemTime,
    registry: Arc<RwLock<CellRegistry>>,
    health_checker: Arc<HealthChecker>,
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

struct HealthChecker {
    checks: HashMap<String, HealthStatus>,
}

struct HealthStatus {
    last_check: std::time::Instant,
    consecutive_failures: u32,
    latency_ms: f64,
}

impl Nucleus {
    pub fn new() -> Self {
        Self {
            start_time: std::time::SystemTime::now(),
            registry: Arc::new(RwLock::new(CellRegistry {
                cells: HashMap::new(),
                last_heartbeat: HashMap::new(),
            })),
            health_checker: Arc::new(HealthChecker {
                checks: HashMap::new(),
            }),
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
                
                reg.last_heartbeat.retain(|_, last| {
                    now.duration_since(*last).as_secs() < 30
                });
                
                let CellRegistry { cells, last_heartbeat } = &mut *reg;
                for instances in cells.values_mut() {
                    instances.retain(|inst| {
                        last_heartbeat.contains_key(&inst.name)
                    });
                }
            }
        });

        let state = self.state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let guard = state.read().await;
                
                if let Some(manifest) = &guard.desired_state {
                    tracing::info!("[Nucleus] Reconciling mesh '{}'...", manifest.mesh);
                    
                    let nodes = Discovery::scan().await;
                    
                    for spec in &manifest.cells {
                        let active_count = nodes.iter()
                            .filter(|n| n.name == spec.name)
                            .count() as u32;

                        if active_count < spec.replicas {
                            let diff = spec.replicas - active_count;
                            tracing::info!("[Nucleus] Scaling up {} (+{} replicas)", spec.name, diff);
                            if let Err(e) = System::spawn(&spec.name, None).await {
                                tracing::error!("Failed to spawn {}: {}", spec.name, e);
                            }
                        }
                    }
                }
            }
        });
    }

    // Fixed function signature using correct types
    fn find_best_node(_placement: &PlacementStrategy, _res: &ResourceLimits) -> Option<String> {
        None
    }

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
            system_health: HealthMetrics {
                cpu_usage: 0.0,
                memory_mb: 0,
                active_connections: 0,
            },
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