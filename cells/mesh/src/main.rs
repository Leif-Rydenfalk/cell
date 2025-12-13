// cells/mesh/src/main.rs
// SPDX-License-Identifier: MIT
// Distributed Mesh Health & Dependency Manager

use cell_sdk::*;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

struct MeshState {
    // cell_name (Consumer) -> set of dependencies (Providers)
    dependency_graph: HashMap<String, HashSet<String>>,
    cell_status: HashMap<String, cell_model::protocol::MeshRequest>, // Simplified placeholder type
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
        
        // Update graph: Record that 'cell_name' depends on 'dependencies'
        state.dependency_graph.insert(cell_name.clone(), dependencies.iter().cloned().collect());
        
        // Resolve sockets
        let mut mapping = HashMap::new();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        
        // We need to check both system and local scopes, but simpler to rely on cell_discovery logic.
        // For the Mesh service, we assume standard system scope for now.
        let socket_dir = home.join(".cell/runtime/system");

        for dep in dependencies {
            let path = socket_dir.join(format!("{}.sock", dep));
            mapping.insert(dep, path.to_string_lossy().to_string());
        }
        
        Ok(mapping)
    }

    async fn report_health(&self, cell_name: String, healthy: bool) -> Result<bool> {
        if healthy {
            // tracing::info!("[Mesh] Cell '{}' is healthy", cell_name);
        } else {
            tracing::warn!("[Mesh] Cell '{}' is unhealthy", cell_name);
        }
        Ok(true)
    }
    
    // NEW: Return the full dependency graph for GC analysis
    async fn get_graph(&self) -> Result<HashMap<String, Vec<String>>> {
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