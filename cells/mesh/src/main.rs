// cells/mesh/src/main.rs
// SPDX-License-Identifier: MIT
// Distributed Mesh Health & Dependency Manager

use cell_sdk::*;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

// === PROTOCOL ===
// We re-use MeshRequest/MeshResponse defined in cell-model via the SDK

#[protein]
pub struct CellStatus {
    pub healthy: bool,
    pub last_heartbeat: u64,
}

// === SERVICE ===

struct MeshState {
    // cell_name -> set of dependencies
    dependency_graph: HashMap<String, HashSet<String>>,
    // cell_name -> status
    cell_status: HashMap<String, CellStatus>,
}

#[service]
#[derive(Clone)]
struct MeshService {
    state: Arc<RwLock<MeshState>>,
}

impl MeshService {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(MeshState {
                dependency_graph: HashMap::new(),
                cell_status: HashMap::new(),
            })),
        }
    }
}

#[handler]
impl MeshService {
    async fn resolve_dependencies(&self, cell_name: String, dependencies: Vec<String>) -> Result<HashMap<String, String>> {
        let mut state = self.state.write().await;
        
        // Update graph
        state.dependency_graph.insert(cell_name.clone(), dependencies.iter().cloned().collect());
        
        // Resolve sockets
        let mut mapping = HashMap::new();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let socket_dir = home.join(".cell/runtime/system");

        for dep in dependencies {
            // In a real impl we'd check if they are running/healthy
            // For now, we return the canonical paths
            let path = socket_dir.join(format!("{}.sock", dep));
            mapping.insert(dep, path.to_string_lossy().to_string());
        }
        
        Ok(mapping)
    }

    async fn report_health(&self, cell_name: String, healthy: bool) -> Result<bool> {
        let mut state = self.state.write().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
            
        state.cell_status.insert(cell_name.clone(), CellStatus {
            healthy,
            last_heartbeat: now,
        });
        
        if healthy {
            tracing::info!("[Mesh] Cell '{}' is healthy", cell_name);
            // In a more complex system, we would trigger start_ready_cells here
        } else {
            tracing::warn!("[Mesh] Cell '{}' is unhealthy", cell_name);
        }
        
        Ok(true)
    }
    
    async fn get_graph(&self) -> Result<Vec<(String, Vec<String>)>> {
        let state = self.state.read().await;
        let graph = state.dependency_graph.iter()
            .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
            .collect();
        Ok(graph)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Mesh] Dependency Manager Active");
    
    let service = MeshService::new();
    service.serve("mesh").await
}