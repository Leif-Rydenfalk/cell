// cells/hypervisor/src/main.rs
// SPDX-License-Identifier: MIT
// The Daemon: System Hypervisor and Process Manager

mod capsid;

use capsid::Capsid;
use cell_sdk::cell_remote;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use cell_model::config::CellInitConfig;
use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error};
use rkyv::Deserialize;

// Remote interface to Builder
cell_remote!(Builder = "builder");

#[cell_sdk::service]
struct HypervisorService;

#[cell_sdk::handler]
impl HypervisorService {
    async fn spawn(&self, cell_name: String, config: Option<CellInitConfig>) -> Result<()> {
        let _ = cell_name;
        let _ = config;
        Ok(())
    }
}

pub struct Hypervisor {
    system_socket_dir: PathBuf,
    daemon_socket_path: PathBuf,
}

impl Hypervisor {
    pub async fn ignite() -> Result<()> {
        let home = dirs::home_dir().expect("No HOME");
        
        let system_socket_dir = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
            PathBuf::from(p)
        } else {
            home.join(".cell/runtime/system")
        };

        tokio::fs::create_dir_all(&system_socket_dir).await?;
        let daemon_socket_path = system_socket_dir.join("mitosis.sock");

        if daemon_socket_path.exists() { tokio::fs::remove_file(&daemon_socket_path).await?; }
        let listener = UnixListener::bind(&daemon_socket_path)?;

        info!("[Hypervisor] Kernel Active. Listening on {:?}", daemon_socket_path);

        let hv = Self { 
            system_socket_dir: system_socket_dir.clone(), 
            daemon_socket_path: daemon_socket_path.clone()
        };

        hv.bootstrap_kernel_cell("builder").await?;
        hv.bootstrap_kernel_cell("nucleus").await?;
        hv.bootstrap_kernel_cell("axon").await?;
        
        let hv_arc = std::sync::Arc::new(hv);
        
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let r_inner = hv_arc.clone();
                tokio::spawn(async move {
                    if let Err(e) = r_inner.handle_request(stream).await {
                        error!("[Hypervisor] Request Error: {}", e);
                    }
                });
            }
        }
    }

    async fn bootstrap_kernel_cell(&self, name: &str) -> Result<()> {
        let socket = self.system_socket_dir.join(format!("{}.sock", name));
        
        if socket.exists() {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                info!("[Hypervisor] {} is already running", name);
                return Ok(());
            }
            tokio::fs::remove_file(&socket).await.ok();
        }

        info!("[Hypervisor] Bootstrapping {}...", name);

        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("run").arg("--release").arg("-p").arg(name);
        
        if let Ok(s) = std::env::var("CELL_SOCKET_DIR") { cmd.env("CELL_SOCKET_DIR", s); }
        if let Ok(r) = std::env::var("CELL_REGISTRY_DIR") { cmd.env("CELL_REGISTRY_DIR", r); }
        if let Ok(h) = std::env::var("HOME") { cmd.env("HOME", h); }
        cmd.env("CELL_NODE_ID", "0");

        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::inherit());

        std::thread::spawn(move || { let _ = cmd.spawn(); });

        for _ in 0..100 {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                info!("[Hypervisor] {} online.", name);
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        anyhow::bail!("Failed to bootstrap {}", name);
    }

    async fn handle_request(&self, mut stream: UnixStream) -> Result<()> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let req = cell_model::rkyv::check_archived_root::<MitosisRequest>(&buf)
            .map_err(|e| anyhow::anyhow!("Protocol Violation: {}", e))?;

        match req {
            cell_model::protocol::ArchivedMitosisRequest::Spawn { cell_name, config } => {
                let name = cell_name.to_string();
                
                let final_config = if let rkyv::option::ArchivedOption::Some(c) = config {
                    c.deserialize(&mut rkyv::Infallible).unwrap()
                } else {
                    let socket_path = self.system_socket_dir.join(format!("{}.sock", name));
                    CellInitConfig {
                        node_id: rand::random(),
                        cell_name: name.clone(),
                        peers: vec![],
                        socket_path: socket_path.to_string_lossy().to_string(),
                        organism: "system".to_string(),
                    }
                };

                match self.perform_spawn(&name, &final_config).await {
                    Ok(_) => {
                        let resp = MitosisResponse::Ok { socket_path: final_config.socket_path };
                        self.send_resp(&mut stream, resp).await?;
                    }
                    Err(e) => {
                        self.send_resp(&mut stream, MitosisResponse::Denied { reason: e.to_string() }).await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn perform_spawn(&self, cell_name: &str, config: &CellInitConfig) -> Result<()> {
        let mut builder = Builder::Client::connect().await
            .context("Hypervisor cannot reach Builder")?;
            
        let build_res = builder.build(Builder::BuildRequest { 
            cell_name: cell_name.to_string() 
        }).await.context("Build failed")?;

        let binary_path = PathBuf::from(build_res.binary_path);

        let socket_path = PathBuf::from(&config.socket_path);
        let runtime_dir = socket_path.parent().unwrap();
        tokio::fs::create_dir_all(runtime_dir).await?;

        Capsid::spawn(&binary_path, runtime_dir, &self.daemon_socket_path, &[], config)?;
        
        Ok(())
    }

    async fn send_resp(&self, stream: &mut UnixStream, resp: MitosisResponse) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    Hypervisor::ignite().await
}