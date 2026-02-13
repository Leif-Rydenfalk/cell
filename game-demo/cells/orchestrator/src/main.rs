//! Cell Orchestrator - Automatic Cell Lifecycle Manager
//!
//! This cell automatically discovers, starts, and manages other cells
//! based on Cell.toml configuration files found in the workspace.

use cell_sdk::*;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use walkdir::WalkDir;
use std::fs;

#[derive(Debug)]
struct ManagedCell {
    name: String,
    path: PathBuf,
    process: Option<Child>,
    autostart: bool,
    status: CellStatus,
}

#[derive(Debug, Clone, PartialEq)]
enum CellStatus {
    Stopped,
    Starting,
    Running,
    Failed(String),
}

struct Orchestrator {
    cells: Arc<RwLock<HashMap<String, ManagedCell>>>,
    workspace_root: PathBuf,
}

impl Orchestrator {
    async fn new(workspace_root: PathBuf) -> Result<Self> {
        Ok(Self {
            cells: Arc::new(RwLock::new(HashMap::new())),
            workspace_root,
        })
    }

    async fn scan_workspace(&self) -> Result<usize> {
        info!("🔍 Scanning workspace: {:?}", self.workspace_root);
        let mut cells = self.cells.write().await;
        let mut count = 0;

        for entry in WalkDir::new(&self.workspace_root)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_name() == "Cell.toml" {
                let cell_dir = entry.path().parent().unwrap().to_path_buf();
                let content = tokio::fs::read_to_string(entry.path()).await?;
                
                // Parse basic Cell.toml
                let mut autostart = false;
                let mut name = None;

                for line in content.lines() {
                    if line.starts_with("name = ") {
                        name = Some(line.trim_start_matches("name = ")
                            .trim_matches('"')
                            .to_string());
                    }
                    if line.contains("autostart = true") {
                        autostart = true;
                    }
                }

                if let Some(name) = name {
                    if !cells.contains_key(&name) && name != "orchestrator" {
                        info!("📦 Found cell: {} at {:?}", name, cell_dir);
                        cells.insert(name.clone(), ManagedCell {
                            name,
                            path: cell_dir,
                            process: None,
                            autostart,
                            status: CellStatus::Stopped,
                        });
                        count += 1;
                    }
                }
            }
        }

        info!("✅ Discovered {} cells", count);
        Ok(count)
    }

    async fn start_cell(&self, name: &str) -> Result<()> {
        let mut cells = self.cells.write().await;
        
        if let Some(cell) = cells.get_mut(name) {
            if cell.status == CellStatus::Running {
                return Ok(());
            }

            info!("🚀 Starting cell: {}", name);
            
            // CRITICAL FIX: Ensure target directory exists and run from correct path
            let target_dir = cell.path.join("target");
            if !target_dir.exists() {
                std::fs::create_dir_all(&target_dir)?;
            }

            // Create .cell/io directory for sockets
            let cell_io_dir = cell.path.join(".cell").join("io");
            std::fs::create_dir_all(&cell_io_dir)?;

            // Also ensure global socket directory exists
            let home = dirs::home_dir().unwrap();
            let global_io_dir = home.join(".cell").join("io");
            std::fs::create_dir_all(&global_io_dir)?;

            // Spawn the process
            let child = Command::new("cargo")
                .args(["run", "--release"])
                .current_dir(&cell.path)  // CRITICAL: Run in the cell's directory, not workspace root
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("Failed to spawn {} from {:?}", name, cell.path))?;

            cell.process = Some(child);
            cell.status = CellStatus::Starting;

            // Wait for socket to appear
            let socket = home.join(".cell/io").join(format!("{}.sock", name));
            info!("Waiting for socket: {:?}", socket);
            
            for i in 0..50 {
                if socket.exists() {
                    cell.status = CellStatus::Running;
                    info!("✅ Cell {} is running (socket found after {}ms)", name, i * 100);
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            cell.status = CellStatus::Failed("Timeout waiting for socket".to_string());
            anyhow::bail!("Cell {} failed to start - socket never appeared at {:?}", name, socket);
        }

        Ok(())
    }

    async fn start_autostart_cells(&self) -> Result<()> {
        let cells = self.cells.read().await;
        let autostart: Vec<String> = cells.values()
            .filter(|c| c.autostart)
            .map(|c| c.name.clone())
            .collect();
        
        info!("🚀 Auto-starting {} cells...", autostart.len());
        
        for name in autostart {
            if let Err(e) = self.start_cell(&name).await {
                error!("Failed to start {}: {}", name, e);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        Ok(())
    }

    async fn health_check_loop(&self) {
        let mut interval = interval(Duration::from_secs(5));
        
        loop {
            interval.tick().await;
            let mut cells = self.cells.write().await;
            
            for (name, cell) in cells.iter_mut() {
                if let Some(ref mut process) = cell.process {
                    match process.try_wait() {
                        Ok(Some(status)) => {
                            warn!("Cell {} exited: {}", name, status);
                            cell.status = CellStatus::Failed("Exited".to_string());
                            cell.process = None;
                        }
                        Ok(None) => {
                            if cell.status == CellStatus::Starting {
                                cell.status = CellStatus::Running;
                            }
                        }
                        Err(e) => error!("Error checking {}: {}", name, e),
                    }
                }
            }
        }
    }

    async fn run(self) -> Result<()> {
        // Scan for cells
        self.scan_workspace().await?;
        
        // Start health checker
        let health_service = self.cells.clone();
        tokio::spawn(async move {
            let orchestrator = Orchestrator {
                cells: health_service,
                workspace_root: PathBuf::new(),
            };
            orchestrator.health_check_loop().await;
        });

        // Start autostart cells
        self.start_autostart_cells().await?;

        info!("✅ Orchestrator ready. Press Ctrl+C to stop.");
        tokio::signal::ctrl_c().await?;
        info!("🛑 Shutting down...");

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();

    println!("🎛️  Cell Orchestrator starting...");

    let workspace_root = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .parent()
        .unwrap_or(&PathBuf::from("."))
        .to_path_buf();

    let orchestrator = Orchestrator::new(workspace_root).await?;
    orchestrator.run().await
}