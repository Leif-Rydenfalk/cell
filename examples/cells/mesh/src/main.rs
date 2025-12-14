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
    // We removed CellStatus logic for brevity if unused, or keep it.
    // Keeping minimal state for dependency tracking.
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
        
        tracing::info!("[Mesh] Recorded dependencies for {}: {:?}", cell_name, dependencies);

        // Resolve sockets (Standard System Logic)
        let mut mapping = HashMap::new();
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let socket_dir = home.join(".cell/runtime/system");

        for dep in dependencies {
            let path = socket_dir.join(format!("{}.sock", dep));
            mapping.insert(dep, path.to_string_lossy().to_string());
        }
        
        Ok(mapping)
    }

    async fn report_health(&self, _cell_name: String, _healthy: bool) -> Result<bool> {
        Ok(true)
    }
    
    // Return HashMap<String, Vec<String>> to match updated Nucleus expectation
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