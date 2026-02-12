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
pub enum BuildMode {
    Standard,
    Test,
}

#[protein]
pub struct BuildRequest {
    pub cell_name: String,
    pub mode: BuildMode,
}

#[protein]
pub struct BuildResponse {
    pub binary_path: String,
    pub source_hash: String, // New field for versioning
}

struct BuilderService {
    registry_path: PathBuf,
}

impl BuilderService {
    fn new() -> Self {
        let home = dirs::home_dir().expect("No HOME dir");
        let registry_path = std::env::var("CELL_REGISTRY_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".cell/registry"));

        fs::create_dir_all(&registry_path).ok();

        Self { registry_path }
    }

    fn resolve_source(&self, cell_name: &str) -> Result<PathBuf> {
        let source_path = self.registry_path.join(cell_name);
        if source_path.exists() {
            return Ok(source_path);
        }
        
        if let Ok(cwd) = std::env::current_dir() {
            let local_cells = cwd.join("cells").join(cell_name);
            if local_cells.exists() { return Ok(local_cells); }
            
            let schema_ex = cwd.join("examples").join("cell-schema-sync").join(cell_name);
            if schema_ex.exists() { return Ok(schema_ex); }
            
            let market_ex = cwd.join("examples").join("cell-market").join(cell_name);
            if market_ex.exists() { return Ok(market_ex); }
        }

        anyhow::bail!("Cell '{}' not found.", cell_name);
    }

    fn build(&self, cell_name: &str, mode: BuildMode) -> Result<(PathBuf, String)> {
        let source_path = self.resolve_source(cell_name)?;

        match mode {
            BuildMode::Standard => Ribosome::synthesize(&source_path, cell_name),
            BuildMode::Test => {
                // For tests, we use a dummy hash or compute one, 
                // but usually tests are one-off. 
                // We'll reuse synthesize logic for consistency or just return empty hash.
                let path = Ribosome::synthesize_test(&source_path, cell_name)?;
                Ok((path, "test-ephemeral".to_string()))
            },
        }
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
        let (path, hash) = self.svc.build(&req.cell_name, req.mode)?;
        Ok(BuildResponse {
            binary_path: path.to_string_lossy().to_string(),
            source_hash: hash,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    info!("[Builder] Compiler Active");
    let service = Builder { svc: std::sync::Arc::new(BuilderService::new()) };
    service.serve("builder").await
}