// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error};
use cell_sdk::protocol::{MitosisRequest, MitosisResponse, ArchivedMitosisRequest};
use cell_sdk::config::CellInitConfig;
use cell_sdk::cell_remote;
use cell_sdk::rkyv::Deserialize;
use cell_sdk::rkyv;

// Define remote interface to talk to Hypervisor
cell_remote!(Hypervisor = "hypervisor");

pub struct MyceliumRoot {
    socket_dir: PathBuf,
}

impl MyceliumRoot {
    pub async fn ignite() -> Result<Self> {
        let home = dirs::home_dir().expect("No HOME");
        
        // System Scope
        let socket_dir = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
            PathBuf::from(p)
        } else {
            home.join(".cell/runtime/system")
        };

        tokio::fs::create_dir_all(&socket_dir).await?;
        let daemon_socket = socket_dir.join("mitosis.sock");

        if daemon_socket.exists() { tokio::fs::remove_file(&daemon_socket).await?; }
        let listener = UnixListener::bind(&daemon_socket)?;

        info!("[Root] Daemon Booting...");

        // 1. Bootstrap Phase
        Self::bootstrap_kernel_cell("builder").await?;
        Self::bootstrap_kernel_cell("hypervisor").await?;

        info!("[Root] Kernel Active. Listening on {:?}", daemon_socket);

        let root = Self { socket_dir };
        let r = root.clone();

        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let r_inner = r.clone();
                    tokio::spawn(async move {
                        if let Err(e) = r_inner.handle_request(stream).await {
                            error!("[Root] Request Error: {}", e);
                        }
                    });
                }
            }
        });

        Ok(root)
    }

    fn clone(&self) -> Self {
        Self { socket_dir: self.socket_dir.clone() }
    }

    /// Rudimentary process spawner for the Kernel Cells (Builder/Hypervisor).
    async fn bootstrap_kernel_cell(name: &str) -> Result<()> {
        use std::process::Command;
        use cell_transport::gap_junction::spawn_with_gap_junction;
        use cell_model::protocol::{MitosisSignal, MitosisControl};
        
        let socket = cell_sdk::resolve_socket_dir().join(format!("{}.sock", name));
        if socket.exists() {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                info!("[Root] {} is already running", name);
                return Ok(());
            }
            tokio::fs::remove_file(&socket).await.ok();
        }

        info!("[Root] Bootstrapping {}...", name);

        let mut cmd = Command::new("cargo");
        cmd.arg("run").arg("--release").arg("-p").arg(name);
        
        if let Ok(s) = std::env::var("CELL_SOCKET_DIR") { cmd.env("CELL_SOCKET_DIR", s); }
        if let Ok(r) = std::env::var("CELL_REGISTRY_DIR") { cmd.env("CELL_REGISTRY_DIR", r); }
        if let Ok(h) = std::env::var("HOME") { cmd.env("HOME", h); }
        cmd.env("CELL_NODE_ID", "0"); 

        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null()); 
        cmd.stderr(std::process::Stdio::inherit());

        let (_child, mut junction) = spawn_with_gap_junction(cmd)?;

        let config = CellInitConfig {
            node_id: 0,
            cell_name: name.to_string(),
            peers: vec![],
            socket_path: socket.to_string_lossy().to_string(),
            organism: "system".to_string(),
        };

        loop {
            match junction.wait_for_signal()? {
                MitosisSignal::RequestIdentity => {
                    junction.send_control(MitosisControl::InjectIdentity(config.clone()))?;
                }
                MitosisSignal::Cytokinesis => {
                    info!("[Root] {} online.", name);
                    break;
                }
                MitosisSignal::Apoptosis { reason } => anyhow::bail!("{} died: {}", name, reason),
                MitosisSignal::Necrosis => anyhow::bail!("{} panicked", name),
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_request(&self, mut stream: UnixStream) -> Result<()> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let req = rkyv::check_archived_root::<MitosisRequest>(&buf)
            .map_err(|e| anyhow::anyhow!("Protocol Violation: {}", e))?;

        let mut hypervisor = Hypervisor::Client::connect().await
            .context("Kernel Panic: Hypervisor unreachable")?;

        match req {
            ArchivedMitosisRequest::Spawn { cell_name, config } => {
                let name = cell_name.to_string();
                
                let final_config = if let rkyv::option::ArchivedOption::Some(c) = config {
                    c.deserialize(&mut rkyv::Infallible).unwrap()
                } else {
                    let socket_path = self.socket_dir.join(format!("{}.sock", name));
                    CellInitConfig {
                        node_id: rand::random(),
                        cell_name: name.clone(),
                        peers: vec![],
                        socket_path: socket_path.to_string_lossy().to_string(),
                        organism: "system".to_string(),
                    }
                };

                match hypervisor.spawn(name, Some(final_config.clone())).await {
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

    async fn send_resp(&self, stream: &mut UnixStream, resp: MitosisResponse) -> Result<()> {
        let bytes = rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }
}