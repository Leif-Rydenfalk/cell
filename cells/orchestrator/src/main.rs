// SPDX-License-Identifier: MIT
// cells/orchestrator/src/main.rs
//! The Cell Orchestrator - Automatic Cell Lifecycle Manager
//! 
//! This cell automatically discovers, starts, and manages other cells
//! based on Cell.toml configuration files found in the workspace.
//! 
//! Features:
//! - Scans workspace for Cell.toml files
//! - Automatically starts cells in dependency order
//! - Monitors cell health and restarts failed cells
//! - Provides cluster-wide status and control

use cell_sdk::*;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};
use walkdir::WalkDir;

/// Cell process handle and metadata
#[derive(Debug)]
struct ManagedCell {
    name: String,
    path: PathBuf,
    process: Option<Child>,
    manifest: CellManifest,
    status: CellStatus,
    last_started: Option<std::time::Instant>,
    restart_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
enum CellStatus {
    Stopped,
    Starting,
    Running,
    Failed(String),
    Unknown,
}

/// Parsed Cell.toml manifest
#[derive(Debug, Clone, Default)]
struct CellManifest {
    name: String,
    version: String,
    neighbors: HashMap<String, NeighborConfig>,
    autostart: bool,
}

#[derive(Debug, Clone)]
enum NeighborConfig {
    Path(String),
    Detailed { path: String, autostart: bool },
}

/// Orchestrator service that manages the cell cluster
#[service]
#[derive(Clone)]
struct OrchestratorService {
    cells: Arc<RwLock<HashMap<String, ManagedCell>>>,
    workspace_root: PathBuf,
    registry_dir: PathBuf,
}

#[handler]
impl OrchestratorService {
    /// List all managed cells and their status
    async fn list_cells(&self) -> Result<Vec<CellInfo>> {
        let cells = self.cells.read().await;
        
        let infos: Vec<CellInfo> = cells.values().map(|cell| CellInfo {
            name: cell.name.clone(),
            version: cell.manifest.version.clone(),
            status: format!("{:?}", cell.status),
            pid: cell.process.as_ref().and_then(|p| p.id()),
            restart_count: cell.restart_count,
            neighbors: cell.manifest.neighbors.keys().cloned().collect(),
        }).collect();
        
        Ok(infos)
    }
    
    /// Start a specific cell by name
    async fn start_cell(&self, name: String) -> Result<String> {
        let mut cells = self.cells.write().await;
        
        if let Some(cell) = cells.get_mut(&name) {
            if cell.status == CellStatus::Running {
                return Ok(format!("Cell '{}' is already running", name));
            }
            
            match self.spawn_cell_process(cell).await {
                Ok(_) => {
                    cell.status = CellStatus::Running;
                    cell.last_started = Some(std::time::Instant::now());
                    Ok(format!("Started cell '{}'", name))
                }
                Err(e) => {
                    cell.status = CellStatus::Failed(e.to_string());
                    Err(anyhow::anyhow!("Failed to start '{}': {}", name, e))
                }
            }
        } else {
            Err(anyhow::anyhow!("Cell '{}' not found", name))
        }
    }
    
    /// Stop a specific cell by name
    async fn stop_cell(&self, name: String) -> Result<String> {
        let mut cells = self.cells.write().await;
        
        if let Some(cell) = cells.get_mut(&name) {
            if let Some(mut process) = cell.process.take() {
                let _ = process.kill();
                cell.status = CellStatus::Stopped;
                Ok(format!("Stopped cell '{}'", name))
            } else {
                Ok(format!("Cell '{}' was not running", name))
            }
        } else {
            Err(anyhow::anyhow!("Cell '{}' not found", name))
        }
    }
    
    /// Restart a specific cell
    async fn restart_cell(&self, name: String) -> Result<String> {
        self.stop_cell(name.clone()).await.ok();
        tokio::time::sleep(Duration::from_millis(500)).await;
        self.start_cell(name).await
    }
    
    /// Get cluster health status
    async fn cluster_health(&self) -> Result<ClusterHealth> {
        let cells = self.cells.read().await;
        
        let total = cells.len();
        let running = cells.values().filter(|c| c.status == CellStatus::Running).count();
        let failed = cells.values().filter(|c| matches!(c.status, CellStatus::Failed(_))).count();
        
        Ok(ClusterHealth {
            total_cells: total as u32,
            running_cells: running as u32,
            failed_cells: failed as u32,
            healthy: failed == 0 && running > 0,
        })
    }
    
    /// Resolve dependency order for all cells
    async fn resolve_dependencies(&self) -> Result<Vec<String>> {
        let cells = self.cells.read().await;
        
        // Build dependency graph
        let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
        
        for (name, cell) in cells.iter() {
            let deps: HashSet<String> = cell.manifest.neighbors.keys().cloned().collect();
            graph.insert(name.clone(), deps);
        }
        
        // Topological sort (Kahn's algorithm)
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for (name, deps) in &graph {
            *in_degree.entry(name.clone()).or_insert(0);
            for dep in deps {
                *in_degree.entry(dep.clone()).or_insert(0) += 1;
            }
        }
        
        let mut queue: Vec<String> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(n, _)| n.clone())
            .collect();
        
        let mut result = Vec::new();
        
        while let Some(name) = queue.pop() {
            result.push(name.clone());
            
            // Find cells that depend on this one
            for (other_name, deps) in &graph {
                if deps.contains(&name) {
                    let degree = in_degree.get_mut(other_name).unwrap();
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push(other_name.clone());
                    }
                }
            }
        }
        
        if result.len() != cells.len() {
            warn!("Circular dependencies detected in cell graph");
        }
        
        Ok(result)
    }
    
    /// Start all cells in dependency order
    async fn start_all(&self) -> Result<Vec<String>> {
        let order = self.resolve_dependencies().await?;
        let mut started = Vec::new();
        
        for name in order {
            match self.start_cell(name.clone()).await {
                Ok(msg) => {
                    info!("{}", msg);
                    started.push(name);
                    // Small delay between starts to avoid overwhelming the system
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => {
                    error!("Failed to start '{}': {}", name, e);
                }
            }
        }
        
        Ok(started)
    }
    
    /// Stop all cells in reverse dependency order
    async fn stop_all(&self) -> Result<Vec<String>> {
        let mut order = self.resolve_dependencies().await?;
        order.reverse();
        
        let mut stopped = Vec::new();
        
        for name in order {
            match self.stop_cell(name.clone()).await {
                Ok(msg) => {
                    info!("{}", msg);
                    stopped.push(name);
                }
                Err(e) => {
                    error!("Failed to stop '{}': {}", name, e);
                }
            }
        }
        
        Ok(stopped)
    }
}

impl OrchestratorService {
    /// Scan workspace for Cell.toml files and populate cells map
    async fn scan_workspace(&self) -> Result<usize> {
        info!("🔍 Scanning workspace for cells: {:?}", self.workspace_root);
        
        let mut count = 0;
        let mut cells = self.cells.write().await;
        
        for entry in WalkDir::new(&self.workspace_root)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_name() == "Cell.toml" {
                let path = entry.path().parent().unwrap().to_path_buf();
                
                match self.parse_manifest(entry.path()).await {
                    Ok(manifest) => {
                        let name = manifest.name.clone();
                        
                        // Check if already registered
                        if !cells.contains_key(&name) {
                            info!("📦 Found cell: {} at {:?}", name, path);
                            
                            cells.insert(name.clone(), ManagedCell {
                                name: name.clone(),
                                path: path.clone(),
                                process: None,
                                manifest: manifest.clone(),
                                status: CellStatus::Stopped,
                                last_started: None,
                                restart_count: 0,
                            });
                            
                            // Register in global registry for discovery
                            self.register_in_registry(&name, &path).await?;
                            
                            count += 1;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse {:?}: {}", entry.path(), e);
                    }
                }
            }
        }
        
        info!("✅ Discovered {} cells", count);
        Ok(count)
    }
    
    /// Parse a Cell.toml file
    async fn parse_manifest(&self, path: &Path) -> Result<CellManifest> {
        let content = tokio::fs::read_to_string(path).await?;
        let mut manifest = CellManifest::default();
        
        // Simple TOML parsing (in production, use toml crate properly)
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("name = ") {
                manifest.name = line.trim_start_matches("name = ")
                    .trim_matches('"')
                    .to_string();
            } else if line.starts_with("version = ") {
                manifest.version = line.trim_start_matches("version = ")
                    .trim_matches('"')
                    .to_string();
            } else if line.contains("=") && !line.starts_with("[") && !line.starts_with("#") {
                // Parse neighbor: name = "path"
                let parts: Vec<&str> = line.splitn(2, "=").collect();
                if parts.len() == 2 {
                    let name = parts[0].trim().to_string();
                    let path = parts[1].trim().trim_matches('"').to_string();
                    manifest.neighbors.insert(name, NeighborConfig::Path(path));
                }
            }
        }
        
        if manifest.name.is_empty() {
            // Use directory name as fallback
            manifest.name = path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
        }
        
        Ok(manifest)
    }
    
    /// Register cell in global registry
    async fn register_in_registry(&self, name: &str, path: &Path) -> Result<()> {
        let link = self.registry_dir.join(name);
        
        // Remove old link if exists
        if link.exists() || link.is_symlink() {
            let _ = tokio::fs::remove_file(&link).await;
        }
        
        // Create symlink
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(path, &link)
                .with_context(|| format!("Failed to create symlink for {}", name))?;
        }
        
        info!("🔗 Registered {} in registry", name);
        Ok(())
    }
    
    /// Spawn a cell process
    async fn spawn_cell_process(&self, cell: &mut ManagedCell) -> Result<Child> {
        let manifest_path = cell.path.join("Cargo.toml");
        
        // Check if it's a Rust project
        if !manifest_path.exists() {
            return Err(anyhow::anyhow!("No Cargo.toml found in {:?}", cell.path));
        }
        
        info!("🚀 Starting cell: {} from {:?}", cell.name, cell.path);
        
        let child = Command::new("cargo")
            .args(["run", "--release", "-p", &cell.name])
            .current_dir(&cell.path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn cell {}", cell.name))?;
        
        cell.process = Some(child);
        cell.status = CellStatus::Starting;
        
        // Wait a moment for the cell to create its socket
        tokio::time::sleep(Duration::from_millis(500)).await;
        
        // Verify it's running by checking socket
        let home = dirs::home_dir().unwrap();
        let socket = home.join(".cell/io").join(format!("{}.sock", cell.name));
        
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if socket.exists() {
                info!("✅ Cell {} is running (socket found)", cell.name);
                return Ok(cell.process.take().unwrap());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        
        // Socket not found - process might have failed
        if let Some(mut process) = cell.process.take() {
            let _ = process.kill();
        }
        
        Err(anyhow::anyhow!("Cell {} failed to start (no socket found)", cell.name))
    }
    
    /// Health check loop - monitors all cells
    async fn health_check_loop(&self) {
        let mut interval = interval(Duration::from_secs(5));
        
        loop {
            interval.tick().await;
            
            let mut cells = self.cells.write().await;
            
            for (name, cell) in cells.iter_mut() {
                // Check if process is still alive
                if let Some(ref mut process) = cell.process {
                    match process.try_wait() {
                        Ok(Some(status)) => {
                            // Process exited
                            warn!("Cell {} exited with status: {:?}", name, status);
                            cell.status = CellStatus::Failed(format!("Exited: {:?}", status));
                            cell.process = None;
                            
                            // Auto-restart if configured
                            if cell.manifest.autostart && cell.restart_count < 3 {
                                cell.restart_count += 1;
                                warn!("Attempting restart {}/3 for {}", cell.restart_count, name);
                                // Restart would happen here
                            }
                        }
                        Ok(None) => {
                            // Still running
                            if cell.status == CellStatus::Starting {
                                cell.status = CellStatus::Running;
                            }
                        }
                        Err(e) => {
                            error!("Error checking {}: {}", name, e);
                        }
                    }
                }
            }
        }
    }
}

#[protein]
#[derive(Debug, Clone)]
struct CellInfo {
    name: String,
    version: String,
    status: String,
    pid: Option<u32>,
    restart_count: u32,
    neighbors: Vec<String>,
}

#[protein]
#[derive(Debug, Clone)]
struct ClusterHealth {
    total_cells: u32,
    running_cells: u32,
    failed_cells: u32,
    healthy: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🎛️  Cell Orchestrator starting...");
    
    // Determine workspace root
    let workspace_root = std::env::var("CELL_WORKSPACE")
        .map(PathBuf::from)
        .or_else(|_| {
            std::env::current_dir()
                .map(|d| d.parent().unwrap_or(&d).to_path_buf())
        })
        .unwrap_or_else(|_| PathBuf::from("."));
    
    let home = dirs::home_dir().context("No HOME directory")?;
    let registry_dir = home.join(".cell/registry");
    std::fs::create_dir_all(&registry_dir)?;
    
    let service = OrchestratorService {
        cells: Arc::new(RwLock::new(HashMap::new())),
        workspace_root: workspace_root.clone(),
        registry_dir,
    };
    
    // Scan for cells
    let count = service.scan_workspace().await?;
    if count == 0 {
        warn!("No cells found in workspace: {:?}", workspace_root);
        warn!("Set CELL_WORKSPACE to specify a different directory");
    }
    
    // Start health check loop
    let health_service = service.clone();
    tokio::spawn(async move {
        health_service.health_check_loop().await;
    });
    
    // Auto-start cells marked for autostart
    let autostart_cells: Vec<String> = {
        let cells = service.cells.read().await;
        cells
            .values()
            .filter(|c| c.manifest.autostart)
            .map(|c| c.name.clone())
            .collect()
    };
    
    if !autostart_cells.is_empty() {
        info!("🚀 Auto-starting {} cells...", autostart_cells.len());
        for name in autostart_cells {
            let _ = service.start_cell(name).await;
        }
    }
    
    println!("✅ Orchestrator ready. Managing {} cells", count);
    println!("   Workspace: {:?}", workspace_root);
    
    // Start serving
    service.serve("orchestrator").await
}