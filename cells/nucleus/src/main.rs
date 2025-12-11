// cells/nucleus/src/main.rs
// SPDX-License-Identifier: MIT
// The Nucleus: System-wide singleton that manages Cell infrastructure

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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

// === NUCLEUS SERVICE ===

pub struct Nucleus {
    start_time: std::time::SystemTime,
    registry: Arc<RwLock<CellRegistry>>,
    health_checker: Arc<HealthChecker>,
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
                for instances in reg.cells.values_mut() {
                    instances.retain(|inst| {
                        reg.last_heartbeat.contains_key(&inst.name)
                    });
                }
            }
        });
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let nucleus = Nucleus::new();
    nucleus.start_background_tasks().await;
    
    println!("[Nucleus] System manager active");
    nucleus.serve("nucleus").await
}