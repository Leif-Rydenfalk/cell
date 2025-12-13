// cells/control-plane/src/main.rs
// The zero-dependency supervisor that manages the entire mesh lifecycle

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

/// Persistent state stored in ~/.cell/control-plane.json
#[derive(Serialize, Deserialize, Default)]
struct MeshState {
    /// Cell name -> ProcessInfo
    processes: HashMap<String, ProcessInfo>,
    /// Dependency graph (consumer -> providers)
    dependencies: HashMap<String, Vec<String>>,
    /// Version hashes
    versions: HashMap<String, String>,
    /// Last health check timestamp
    last_health: HashMap<String, u64>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ProcessInfo {
    pid: u32,
    socket_path: String,
    version_hash: String,
    start_time: u64,
    restart_count: u32,
}

/// The omniscient control plane
struct ControlPlane {
    state: MeshState,
    state_file: PathBuf,
    running: HashMap<String, Child>,
    boot_order: Vec<&'static str>,
}

impl ControlPlane {
    fn new() -> Self {
        let home = dirs::home_dir().expect("No HOME");
        let state_file = home.join(".cell/control-plane.json");
        
        let state = if state_file.exists() {
            serde_json::from_str(&std::fs::read_to_string(&state_file).unwrap())
                .unwrap_or_default()
        } else {
            MeshState::default()
        };

        Self {
            state,
            state_file,
            running: HashMap::new(),
            boot_order: vec![
                "builder",      // Must be first (compiles everything)
                "hypervisor",   // Process manager
                "nucleus",      // Service registry
                "mesh",         // Dependency graph
                "axon",         // Network gateway
                "observer",     // Monitoring
            ],
        }
    }

    /// PHASE 1: Bootstrap the kernel cells
    async fn bootstrap_kernel(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("PHASE 1: Bootstrapping kernel cells...\n");

        for cell in &self.boot_order {
            println!("  ├─ Starting {}...", cell);
            
            // Check if already running and healthy
            if self.is_running_and_healthy(cell).await {
                println!("  │  └─ ✓ Already running");
                continue;
            }

            // Kill stale process if exists
            if let Some(info) = self.state.processes.get(*cell) {
                self.kill_process(info.pid);
            }

            // Spawn new process
            match self.spawn_cell(cell).await {
                Ok(child) => {
                    let pid = child.id();
                    self.running.insert(cell.to_string(), child);
                    
                    self.state.processes.insert(cell.to_string(), ProcessInfo {
                        pid,
                        socket_path: self.socket_path(cell),
                        version_hash: "kernel".to_string(),
                        start_time: Self::now(),
                        restart_count: 0,
                    });
                    
                    println!("  │  └─ ✓ Started (PID {})", pid);
                    
                    // Wait for readiness
                    self.wait_for_ready(cell, Duration::from_secs(10)).await?;
                    println!("  │  └─ ✓ Ready");
                }
                Err(e) => {
                    eprintln!("  │  └─ ✗ Failed: {}", e);
                    return Err(e);
                }
            }
        }

        self.persist_state()?;
        println!("\n✓ Kernel online\n");
        Ok(())
    }

    /// PHASE 2: Start application cells based on dependencies
    async fn start_applications(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("PHASE 2: Starting application cells...\n");

        // Load dependency graph from Mesh
        self.refresh_dependency_graph().await?;

        // Topological sort for correct startup order
        let order = self.topological_sort();

        for cell in order {
            if self.boot_order.contains(&cell.as_str()) {
                continue; // Skip kernel cells
            }

            println!("  ├─ Starting {}...", cell);
            match self.ensure_running(&cell).await {
                Ok(_) => println!("  │  └─ ✓ Started"),
                Err(e) => eprintln!("  │  └─ ⚠ Failed: {}", e),
            }
        }

        println!("\n✓ Applications online\n");
        Ok(())
    }

    /// PHASE 3: Continuous health monitoring
    async fn monitor_health(&mut self) -> ! {
        println!("PHASE 3: Health monitoring active\n");
        
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;

            let mut unhealthy = Vec::new();

            for (cell, info) in &self.state.processes {
                if !self.is_process_alive(info.pid) {
                    unhealthy.push(cell.clone());
                }
            }

            for cell in unhealthy {
                println!("⚠ {} died, restarting...", cell);
                if let Err(e) = self.restart_cell(&cell).await {
                    eprintln!("  └─ Restart failed: {}", e);
                }
            }

            // Check for version updates
            self.check_for_updates().await;
        }
    }

    /// PHASE 4: Graceful shutdown
    async fn shutdown_all(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        println!("\n🛑 Shutting down mesh...\n");

        // Reverse topological order for graceful shutdown
        let mut order = self.topological_sort();
        order.reverse();

        for cell in order {
            println!("  ├─ Stopping {}...", cell);
            if let Some(info) = self.state.processes.get(&cell) {
                self.graceful_shutdown(&cell, info.pid).await;
            }
        }

        // Kernel cells last
        for cell in self.boot_order.iter().rev() {
            println!("  ├─ Stopping {}...", cell);
            if let Some(info) = self.state.processes.get(*cell) {
                self.graceful_shutdown(cell, info.pid).await;
            }
        }

        self.state.processes.clear();
        self.persist_state()?;
        
        println!("\n✓ Mesh stopped\n");
        Ok(())
    }

    // --- HELPER METHODS ---

    async fn spawn_cell(&self, name: &str) -> Result<Child, Box<dyn std::error::Error>> {
        let mut cmd = Command::new("cargo");
        cmd.args(&["run", "--release", "-p", name]);
        cmd.env("CELL_SOCKET_DIR", self.socket_dir());
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::inherit());
        
        Ok(cmd.spawn()?)
    }

    async fn ensure_running(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_running_and_healthy(name).await {
            return Ok(());
        }

        let child = self.spawn_cell(name).await?;
        let pid = child.id();
        
        self.running.insert(name.to_string(), child);
        self.state.processes.insert(name.to_string(), ProcessInfo {
            pid,
            socket_path: self.socket_path(name),
            version_hash: "unknown".to_string(),
            start_time: Self::now(),
            restart_count: 0,
        });

        self.wait_for_ready(name, Duration::from_secs(5)).await?;
        self.persist_state()?;
        Ok(())
    }

    async fn restart_cell(&mut self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(mut info) = self.state.processes.get_mut(name) {
            self.kill_process(info.pid);
            info.restart_count += 1;
        }

        // Exponential backoff
        let wait = if let Some(info) = self.state.processes.get(name) {
            Duration::from_secs(2u64.pow(info.restart_count.min(5)))
        } else {
            Duration::from_secs(2)
        };

        tokio::time::sleep(wait).await;
        self.ensure_running(name).await
    }

    async fn check_for_updates(&mut self) {
        // Query Builder for latest version hashes
        // Compare with running versions
        // Trigger hot-swap if needed
    }

    async fn graceful_shutdown(&self, name: &str, pid: u32) {
        // Send Shutdown OPS command via socket
        // Wait up to 5 seconds
        // SIGTERM if not responsive
        // SIGKILL as last resort
        
        tokio::time::timeout(
            Duration::from_secs(5),
            self.send_shutdown_signal(name)
        ).await.ok();

        if self.is_process_alive(pid) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM
            );
            
            tokio::time::sleep(Duration::from_secs(2)).await;
            
            if self.is_process_alive(pid) {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGKILL
                );
            }
        }
    }

    async fn is_running_and_healthy(&self, name: &str) -> bool {
        if let Some(info) = self.state.processes.get(name) {
            if !self.is_process_alive(info.pid) {
                return false;
            }
            
            // Try connecting to socket
            let socket = PathBuf::from(&info.socket_path);
            tokio::net::UnixStream::connect(socket).await.is_ok()
        } else {
            false
        }
    }

    fn is_process_alive(&self, pid: u32) -> bool {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            None
        ).is_ok()
    }

    fn kill_process(&self, pid: u32) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGKILL
        );
    }

    async fn wait_for_ready(&self, name: &str, timeout: Duration) -> Result<(), Box<dyn std::error::Error>> {
        let socket = PathBuf::from(self.socket_path(name));
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            if socket.exists() {
                if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Err("Timeout waiting for cell readiness".into())
    }

    async fn send_shutdown_signal(&self, name: &str) -> Result<(), Box<dyn std::error::Error>> {
        // Connect and send OPS::Shutdown
        Ok(())
    }

    async fn refresh_dependency_graph(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Query Mesh cell for full dependency graph
        // Update self.state.dependencies
        Ok(())
    }

    fn topological_sort(&self) -> Vec<String> {
        // Kahn's algorithm for dependency resolution
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut result = Vec::new();

        // Build in-degree map
        for (consumer, providers) in &self.state.dependencies {
            in_degree.entry(consumer.clone()).or_insert(0);
            for provider in providers {
                *in_degree.entry(provider.clone()).or_insert(0) += 1;
            }
        }

        // Start with nodes that have no dependencies
        let mut queue: Vec<_> = in_degree.iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(name, _)| name.clone())
            .collect();

        while let Some(cell) = queue.pop() {
            result.push(cell.clone());

            if let Some(providers) = self.state.dependencies.get(&cell) {
                for provider in providers {
                    if let Some(deg) = in_degree.get_mut(provider) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(provider.clone());
                        }
                    }
                }
            }
        }

        result
    }

    fn persist_state(&self) -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_string_pretty(&self.state)?;
        std::fs::write(&self.state_file, json)?;
        Ok(())
    }

    fn socket_dir(&self) -> String {
        dirs::home_dir()
            .unwrap()
            .join(".cell/runtime/system")
            .to_string_lossy()
            .to_string()
    }

    fn socket_path(&self, name: &str) -> String {
        format!("{}/{}.sock", self.socket_dir(), name)
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cp = ControlPlane::new();

    // Handle signals
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        println!("\n^C received");
        std::process::exit(0);
    });

    // Execute lifecycle phases
    cp.bootstrap_kernel().await?;
    cp.start_applications().await?;
    cp.monitor_health().await; // Never returns

    Ok(())
}