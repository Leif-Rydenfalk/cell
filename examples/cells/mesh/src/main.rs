// cells/mesh/src/main.rs
// SPDX-License-Identifier: MIT
// The Mesh Cell: Tracks dependency graph and health.

use cell_sdk::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use anyhow::Result;

#[protein]
pub struct ResolveRequest {
    pub cell_name: String,
    pub dependencies: Vec<String>,
}

#[protein]
pub struct HealthReport {
    pub cell_name: String,
    pub healthy: bool,
}

// Protocol expected by Nucleus and others
#[protein]
pub enum MeshRequest {
    ResolveDependencies { cell_name: String, dependencies: Vec<String> },
    ReportHealth { cell_name: String, healthy: bool },
    GetFullGraph,
}

#[protein]
pub enum MeshResponse {
    DependencyMapping { cell_name: String, socket_paths: HashMap<String, String> },
    Ack,
    FullGraph(HashMap<String, Vec<String>>),
    Error { message: String },
}

#[service]
struct MeshService {
    // consumer -> providers
    graph: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

#[handler]
impl MeshService {
    // Standard handler pattern matching the Enum dispatch in generated code
    // For this specific cell, we implement the methods corresponding to the protocol 
    // defined in `cell-model` or implied by usage.
    
    // The macro generates a client that calls methods. 
    // We implement the methods here.

    async fn resolve_dependencies(&self, cell_name: String, dependencies: Vec<String>) -> Result<HashMap<String, String>> {
        let mut g = self.graph.lock().unwrap();
        g.insert(cell_name.clone(), dependencies.clone());
        tracing::info!("Registered dependencies for {}: {:?}", cell_name, dependencies);
        Ok(HashMap::new()) // In a real mesh, we'd return resolved paths
    }

    async fn report_health(&self, cell_name: String, healthy: bool) -> Result<()> {
        tracing::debug!("Health report for {}: {}", cell_name, healthy);
        Ok(())
    }

    async fn get_graph(&self) -> Result<HashMap<String, Vec<String>>> {
        Ok(self.graph.lock().unwrap().clone())
    }
}

// Manual dispatch glue to match cell-model expectations if strictly needed, 
// but since we are using `cell_remote` on THIS source file, the client will match THIS implementation.
// The `nucleus` cell uses `cell_remote!(Mesh = "mesh")`, so it sees these methods.

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("Mesh Service Online");

    let service = MeshService {
        graph: Arc::new(Mutex::new(HashMap::new())),
    };

    service.serve("mesh").await
}