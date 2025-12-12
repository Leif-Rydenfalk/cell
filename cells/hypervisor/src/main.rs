// cells/hypervisor/src/main.rs
// SPDX-License-Identifier: MIT
// The Capsid: Process Isolation & Management

use cell_sdk::*;
use cell_model::config::CellInitConfig;
use anyhow::{Result, Context};
use std::process::{Command, Stdio};
use std::path::{Path, PathBuf};
use std::io::Write;
use tracing::{info, error};

// Define Remote Interface for Builder
cell_remote!(Builder = "builder");

#[protein]
pub struct SpawnRequest {
    pub cell_name: String,
    pub config: CellInitConfig,
}

#[protein]
pub struct SpawnResponse {
    pub pid: u32,
    pub socket_path: String,
}

struct HypervisorService {
    umbilical_path: PathBuf,
}

impl HypervisorService {
    fn new() -> Self {
        // We need to know where the Daemon is listening to pass it to children
        // The Daemon sets CELL_SOCKET_DIR for the Hypervisor.
        let socket_dir = std::env::var("CELL_SOCKET_DIR").expect("CELL_SOCKET_DIR not set");
        let umbilical_path = PathBuf::from(socket_dir).join("mitosis.sock");
        Self { umbilical_path }
    }

    async fn spawn_process(&self, binary: PathBuf, config: &CellInitConfig) -> Result<u32> {
        let socket_path = PathBuf::from(&config.socket_path);
        let socket_dir = socket_path.parent().unwrap();
        
        tokio::fs::create_dir_all(socket_dir).await?;

        // 1. Sandbox Construction (bwrap)
        let mut cmd = Command::new("bwrap");
        
        cmd.arg("--unshare-all")
            .arg("--share-net")
            .arg("--die-with-parent")
            .arg("--new-session")
            .arg("--cap-drop").arg("ALL")
            // Filesystem
            .arg("--ro-bind").arg("/usr").arg("/usr")
            .arg("--ro-bind").arg("/bin").arg("/bin")
            .arg("--ro-bind").arg("/sbin").arg("/sbin")
            .arg("--dev").arg("/dev")
            .arg("--proc").arg("/proc")
            .arg("--tmpfs").arg("/tmp")
            // Bind Runtime
            .arg("--bind").arg(socket_dir).arg("/tmp/cell")
            .arg("--bind").arg(&self.umbilical_path).arg("/tmp/mitosis.sock")
            // Bind Binary
            .arg("--ro-bind").arg(&binary).arg("/tmp/dna/payload");

        if Path::new("/lib").exists() { cmd.arg("--ro-bind").arg("/lib").arg("/lib"); }
        if Path::new("/lib64").exists() { cmd.arg("--ro-bind").arg("/lib64").arg("/lib64"); }

        // Environment
        cmd.env("CELL_SOCKET_DIR", "/tmp/cell");
        cmd.env("CELL_UMBILICAL", "/tmp/mitosis.sock");
        cmd.env("CELL_ORGANISM", &config.organism);
        cmd.env_remove("CELL_NODE_ID"); // Injected via STDIN

        // Exec
        cmd.arg("/tmp/dna/payload");
        cmd.stdin(Stdio::piped());
        // cmd.stdout(Stdio::inherit());
        // cmd.stderr(Stdio::inherit());

        let mut child = cmd.spawn().context("Failed to spawn bwrap")?;

        // 2. Injection (Umbilical Cord)
        if let Some(mut stdin) = child.stdin.take() {
            let bytes = cell_model::rkyv::to_bytes::<_, 1024>(config)?.into_vec();
            let len = (bytes.len() as u32).to_le_bytes();
            stdin.write_all(&len)?;
            stdin.write_all(&bytes)?;
            stdin.flush()?;
        }

        Ok(child.id())
    }
}

#[service]
#[derive(Clone)]
struct Hypervisor {
    svc: std::sync::Arc<HypervisorService>,
}

#[handler]
impl Hypervisor {
    async fn spawn(&self, req: SpawnRequest) -> Result<SpawnResponse> {
        info!("[Hypervisor] Request to spawn '{}' in '{}'", req.cell_name, req.config.organism);

        // 1. Get Binary from Builder
        // We connect to the Builder cell to get the path to the executable
        let mut builder = Builder::Client::connect().await
            .context("Failed to connect to Builder")?;
            
        let build_res = builder.build(Builder::BuildRequest { 
            cell_name: req.cell_name.clone() 
        }).await.context("Build failed")?;

        let binary_path = PathBuf::from(build_res.binary_path);

        // 2. Spawn Process
        let pid = self.svc.spawn_process(binary_path, &req.config).await?;

        info!("[Hypervisor] Spawned PID {}", pid);

        Ok(SpawnResponse {
            pid,
            socket_path: req.config.socket_path.clone(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Hypervisor] Process Manager Active");
    
    // Ensure we bind to the system scope (mitosis.sock is implicit here for clients)
    // Actually, the Root spawns us. We serve "hypervisor".
    // System::spawn connects to "mitosis.sock" which is the Root listening? 
    // OR does the Root just forward? 
    // 
    // REVISION: The Root process acts as the initial listener on `mitosis.sock`. 
    // It forwards requests to the Hypervisor cell.
    // So Hypervisor just serves on its own socket "hypervisor.sock".
    
    let svc = HypervisorService::new();
    let service = Hypervisor { svc: std::sync::Arc::new(svc) };
    service.serve("hypervisor").await
}