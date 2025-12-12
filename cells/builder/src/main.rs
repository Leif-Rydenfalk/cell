// cells/builder/src/main.rs
// SPDX-License-Identifier: MIT
// The Ribosome: Compiles DNA (Source) into Proteins (Binaries)

mod ribosome;

use cell_sdk::*;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::fs;
use tracing::{info, error};
use ribosome::Ribosome;

#[protein]
pub struct BuildRequest {
    pub cell_name: String,
}

#[protein]
pub struct BuildResponse {
    pub binary_path: String,
}

struct BuilderService {
    registry_path: PathBuf,
}

impl BuilderService {
    fn new() -> Self {
        let home = dirs::home_dir().expect("No HOME dir");
        // Env overrides for testing/isolation
        let registry_path = std::env::var("CELL_REGISTRY_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".cell/registry"));
        
        fs::create_dir_all(&registry_path).ok();
        
        Self {
            registry_path,
        }
    }

    fn build(&self, cell_name: &str) -> Result<PathBuf> {
        let source_path = self.registry_path.join(cell_name);
        if !source_path.exists() {
            anyhow::bail!("Cell '{}' not found in registry at {:?}", cell_name, source_path);
        }
        
        // Delegate to Ribosome module logic
        Ribosome::synthesize(&source_path, cell_name)
    }
}

#[service]
#[derive(Clone)]
struct Builder {
    svc: std::sync::Arc<BuilderService>,
}

#[handler]
impl Builder {
    async fn build(&self, req: BuildRequest) -> Result<BuildResponse> {
        let path = self.svc.build(&req.cell_name)?;
        Ok(BuildResponse {
            binary_path: path.to_string_lossy().to_string(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Builder] Compiler Active");
    let service = Builder { svc: std::sync::Arc::new(BuilderService::new()) };
    service.serve("builder").await
}