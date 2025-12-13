// cells/builder/src/main.rs
// SPDX-License-Identifier: MIT
// The Ribosome: Compiles DNA (Source) into Proteins (Binaries)

mod ribosome;

use anyhow::Result;
use cell_sdk::*;
use ribosome::Ribosome;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{error, info};

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

        Self { registry_path }
    }

    fn build(&self, cell_name: &str) -> Result<PathBuf> {
        // 1. Try Registry
        let mut source_path = self.registry_path.join(cell_name);
        
        // 2. Fallback: Workspace Search
        // If the Builder is running inside a cargo workspace, check sibling directories
        if !source_path.exists() {
            if let Ok(cwd) = std::env::current_dir() {
                // Try cells/{name}
                let local_cells = cwd.join("cells").join(cell_name);
                if local_cells.exists() {
                    info!("[Builder] Found '{}' in local workspace cells/", cell_name);
                    source_path = local_cells;
                } else {
                    // Try examples/cell-schema-sync/{name}
                    let schema_ex = cwd.join("examples").join("cell-schema-sync").join(cell_name);
                    if schema_ex.exists() {
                        info!("[Builder] Found '{}' in examples/cell-schema-sync/", cell_name);
                        source_path = schema_ex;
                    }
                }
            }
        }

        if !source_path.exists() {
            anyhow::bail!(
                "Cell '{}' not found in registry ({:?}) or local workspace.",
                cell_name,
                self.registry_path
            );
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
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    info!("[Builder] Compiler Active");

    // Identity hydration is handled by Runtime via Gap Junction

    let service = Builder {
        svc: std::sync::Arc::new(BuilderService::new()),
    };
    service.serve("builder").await
}