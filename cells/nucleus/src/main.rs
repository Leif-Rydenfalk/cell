// cells/nucleus/src/main.rs
// SPDX-License-Identifier: MIT
// The Nucleus: System-wide singleton that manages Cell infrastructure

use cell_sdk::*;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use cell_discovery::{Discovery, LanDiscovery, hardware::HardwareCaps};
use cell_model::manifest::MeshManifest;

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
    pub required_caps: String, // e.g. "gpu"
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
    spores: HashMap<String, Vec<u8>>, // Spore ID -> Binary
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
        
        // Heartbeat monitor
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let mut reg = registry.write().await;
                let now = std::time::Instant::now();
                
                // Remove stale cells (no heartbeat in 30s)
                reg.last_heartbeat.retain(|_, last| {
                    now.duration_since(*last).as_secs() < 30
                });
                
                // Prune dead instances
                // Fix: Destructure to split the borrow. `cells` and `last_heartbeat` are disjoint fields.
                let CellRegistry { cells, last_heartbeat } = &mut *reg;

                for instances in cells.values_mut() {
                    instances.retain(|inst| {
                        last_heartbeat.contains_key(&inst.name)
                    });
                }
            }
        });

        // The Control Loop (Converges State)
        let state = self.state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let guard = state.read().await;
                
                if let Some(manifest) = &guard.desired_state {
                    tracing::info!("[Nucleus] Reconciling mesh '{}'...", manifest.mesh);
                    
                    let nodes = Discovery::scan().await;
                    
                    for spec in &manifest.cells {
                        // Check active replicas
                        let active_count = nodes.iter()
                            .filter(|n| n.name == spec.name)
                            .count() as u32;

                        if active_count < spec.replicas {
                            let diff = spec.replicas - active_count;
                            tracing::info!("[Nucleus] Scaling up {} (+{} replicas)", spec.name, diff);
                            
                            // Hardware-Aware Placement
                            let _best_node = Self::find_best_node(&spec.placement, &spec.resources);
                            
                            // Trigger Spawn via Hypervisor
                            // In a real cluster, this RPCs to the remote hypervisor on 'best_node'.
                            // Here we default to local if not found.
                            if let Err(e) = System::spawn(&spec.name, None).await {
                                tracing::error!("Failed to spawn {}: {}", spec.name, e);
                            }
                        }
                    }
                }
            }
        });
    }

    fn find_best_node(_placement: &cell_model::manifest::PlacementStrategy, _res: &cell_model::manifest::ResourceLimits) -> Option<String> {
        // Access global LAN cache
        // Filter by HardwareCaps (GPU, TEE, AVX)
        // Sort by Thermal Headroom (thermal_zone_temp)
        None // Placeholder implementation
    }
}

#[handler]
impl Nucleus {
    pub async fn register(&self, reg: CellRegistration) -> Result<bool> {
        let mut registry = self.registry.write().await;
        
        registry.last_heartbeat.insert(
            reg.name.clone(),
            std::time::Instant::now()
        );
        
        registry.cells
            .entry(reg.name.clone())
            .or_insert_with(Vec::new)
            .push(reg);
        
        Ok(true)
    }

    pub async fn discover(&self, query: DiscoveryQuery) -> Result<DiscoveryResult> {
        let registry = self.registry.read().await;
        
        let instances = registry.cells
            .get(&query.cell_name)
            .map(|regs| {
                regs.iter()
                    .map(|r| CellInstance {
                        node_id: r.node_id,
                        address: r.endpoints.first().cloned().unwrap_or_default(),
                        latency_us: 0,
                        health_score: 1.0,
                    })
                    .collect()
            })
            .unwrap_or_default();
        
        Ok(DiscoveryResult { instances })
    }

    pub async fn status(&self) -> Result<NucleusStatus> {
        let uptime = std::time::SystemTime::now()
            .duration_since(self.start_time)
            .unwrap_or_default()
            .as_secs();
        
        let registry = self.registry.read().await;
        let managed_cells: Vec<String> = registry.cells.keys().cloned().collect();
        
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
        registry.last_heartbeat.insert(cell_name, std::time::Instant::now());
        Ok(true)
    }

    async fn apply(&self, req: ApplyManifest) -> Result<bool> {
        let manifest: MeshManifest = serde_yaml::from_str(&req.yaml)
            .map_err(|e| anyhow!("Invalid YAML: {}", e))?;
        
        let mut state = self.state.write().await;
        state.desired_state = Some(manifest.clone());
        
        tracing::info!("[Nucleus] Applied manifest for mesh '{}'", manifest.mesh);
        Ok(true)
    }

    async fn schedule(&self, req: ScheduleSpore) -> Result<String> {
        // Lattice Scheduling Logic
        // 1. Find node with lowest load + matching caps
        // 2. Return address of that node
        tracing::info!("[Nucleus] Scheduling spore '{}' on GPU node...", req.spore_id);
        Ok("127.0.0.1:9000".to_string())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let nucleus = Nucleus::new();
    nucleus.start_background_tasks().await;
    
    println!("[Nucleus] System manager active");
    nucleus.serve("nucleus").await
}