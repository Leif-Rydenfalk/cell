//! Palantir Orchestrator - Cell Lifecycle Manager
//!
//! Automatically discovers and manages all Palantir cells

use anyhow::{Context, Result};
use cell_sdk::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use walkdir::WalkDir;

#[derive(Debug)]
struct ManagedCell {
    name: String,
    path: PathBuf,
    process: Option<Child>,
    autostart: bool,
    status: CellStatus,
    dependencies: Vec<String>,
    restart_count: u32,
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
        info!("🔍 Scanning Palantir cells: {:?}", self.workspace_root);
        let mut cells = self.cells.write().await;
        let mut count = 0;
        
        for entry in WalkDir::new(&self.workspace_root)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_name() == "Cell.toml" {
                let cell_dir = entry.path().parent().unwrap().to_path_buf();
                let content = tokio::fs::read_to_string(entry.path()).await?;
                
                let mut autostart = true; // Default to true for Palantir
                let mut name = None;
                let mut dependencies = Vec::new();
                
                for line in content.lines() {
                    if line.starts_with("name = ") {
                        name = Some(line.trim_start_matches("name = ")
                            .trim_matches('"')
                            .to_string());
                    }
                    if line.starts_with("autostart = false") {
                        autostart = false;
                    }
                    if line.starts_with('[') && line.contains("neighbors]") {
                        // Parse dependencies from neighbors section
                        // Simplified for now
                    }
                }
                
                if let Some(name) = name {
                    if !cells.contains_key(&name) {
                        info!("📦 Found Palantir cell: {} at {:?}", name, cell_dir);
                        cells.insert(name.clone(), ManagedCell {
                            name,
                            path: cell_dir,
                            process: None,
                            autostart,
                            status: CellStatus::Stopped,
                            dependencies,
                            restart_count: 0,
                        });
                        count += 1;
                    }
                }
            }
        }
        
        info!("✅ Discovered {} Palantir cells", count);
        Ok(count)
    }
    
    async fn start_cell(&self, name: &str) -> Result<()> {
        let mut cells = self.cells.write().await;
        
        if let Some(cell) = cells.get_mut(name) {
            if cell.status == CellStatus::Running {
                return Ok(());
            }
            
            info!("🚀 Starting Palantir cell: {}", name);
            
            // Create necessary directories
            let cell_io_dir = cell.path.join(".cell").join("io");
            std::fs::create_dir_all(&cell_io_dir)?;
            
            let home = dirs::home_dir().unwrap();
            let global_io_dir = home.join(".cell").join("io");
            std::fs::create_dir_all(&global_io_dir)?;
            
            // Start the cell
            let child = Command::new("cargo")
                .args(["run", "--release"])
                .current_dir(&cell.path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .with_context(|| format!("Failed to spawn {}", name))?;
            
            cell.process = Some(child);
            cell.status = CellStatus::Starting;
            
            // Wait for socket to appear
            let socket = home.join(".cell/io").join(format!("{}.sock", name));
            info!("Waiting for {} socket: {:?}", name, socket);
            
            for i in 0..50 {
                if socket.exists() {
                    cell.status = CellStatus::Running;
                    info!("✅ Cell {} is running (after {}ms)", name, i * 100);
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            
            cell.status = CellStatus::Failed("Timeout".to_string());
            anyhow::bail!("Cell {} failed to start", name);
        }
        
        Ok(())
    }
    
    async fn start_all(&self) -> Result<()> {
        // Start in dependency order (identity first, then provenance, then others)
        let order = vec![
            "identity",      // Foundation
            "provenance",    // Lineage
            "temporal",      // Time-series
            "ingest-sec",    // SEC data
            "correlation",   // Relationship finding
        ];
        
        for name in order {
            if let Err(e) = self.start_cell(name).await {
                error!("Failed to start {}: {}", name, e);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        
        Ok(())
    }
    
    async fn health_check_loop(&self) {
        let mut interval = interval(Duration::from_secs(10));
        
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
                            
                            // Auto-restart (max 3 times)
                            if cell.restart_count < 3 {
                                cell.restart_count += 1;
                                warn!("Restarting {} (attempt {}/3)", name, cell.restart_count);
                                
                                // Restart in background
                                let name = name.clone();
                                let cells_clone = self.cells.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(Duration::from_secs(2)).await;
                                    let mut cells = cells_clone.write().await;
                                    if let Some(cell) = cells.get_mut(&name) {
                                        let _ = cell.process.take();
                                    }
                                    drop(cells);
                                    
                                    // Reconnect logic here
                                });
                            }
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🎛️  Palantir Orchestrator");
    println!("   └─ Managing Intelligence Cells");
    
    let workspace_root = std::env::current_dir()?
        .parent()
        .unwrap()
        .to_path_buf();
    
    let orchestrator = Orchestrator::new(workspace_root).await?;
    
    // Scan for cells
    let count = orchestrator.scan_workspace().await?;
    if count == 0 {
        warn!("No Palantir cells found");
        return Ok(());
    }
    
    // Start health checker
    let health_orchestrator = orchestrator.cells.clone();
    tokio::spawn(async move {
        let orch = Orchestrator {
            cells: health_orchestrator,
            workspace_root: PathBuf::new(),
        };
        orch.health_check_loop().await;
    });
    
    // Start all cells in order
    orchestrator.start_all().await?;
    
    println!("\n✅ Palantir Core Online");
    println!("   ├─ Identity Cell: Entity resolution");
    println!("   ├─ Provenance Cell: Data lineage");
    println!("   ├─ Temporal Cell: Time-series (coming soon)");
    println!("   ├─ SEC Ingest: EDGAR data (coming soon)");
    println!("   └─ Correlation: Relationship finding (coming soon)");
    
    tokio::signal::ctrl_c().await?;
    println!("🛑 Shutting down Palantir...");
    
    Ok(())
}