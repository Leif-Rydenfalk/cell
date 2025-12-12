// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::capsid::Capsid;
use crate::ribosome::Ribosome;
use cell_model::protocol::{MitosisRequest, MitosisResponse, ArchivedMitosisRequest};
use cell_model::config::CellInitConfig;
use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, error};
use rkyv::option::ArchivedOption;
use rkyv::Deserialize;

pub struct MyceliumRoot {
    // Root always binds to system scope
    system_socket_dir: PathBuf,
    registry_path: PathBuf,
    umbilical_path: PathBuf,
}

impl MyceliumRoot {
    pub async fn ignite() -> Result<Self> {
        // Root always lives in the 'system' runtime directory unless forced by CELL_SOCKET_DIR
        let system_socket_dir = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
            PathBuf::from(p)
        } else {
            let home = dirs::home_dir().context("Home dir not found")?;
            home.join(".cell/runtime/system")
        };

        let registry_path = if let Ok(p) = std::env::var("CELL_REGISTRY_DIR") {
            PathBuf::from(p)
        } else {
            let home = dirs::home_dir().context("Home dir not found")?;
            home.join(".cell/registry")
        };

        let umbilical_path = system_socket_dir.join("mitosis.sock");

        tokio::fs::create_dir_all(&system_socket_dir).await?;
        tokio::fs::create_dir_all(&registry_path).await?;

        if umbilical_path.exists() { tokio::fs::remove_file(&umbilical_path).await?; }
        let listener = UnixListener::bind(&umbilical_path)?;
        
        info!("[Root] System Hypervisor Active at {:?}", umbilical_path);

        let root = Self { system_socket_dir, registry_path, umbilical_path };
        let r = root.clone();
        
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let mut r_inner = r.clone();
                    tokio::spawn(async move {
                        if let Err(e) = r_inner.handle_child(stream).await {
                            error!("[Root] Spawn Error: {}", e);
                        }
                    });
                }
            }
        });

        Ok(root)
    }

    fn clone(&self) -> Self {
        Self { 
            system_socket_dir: self.system_socket_dir.clone(), 
            registry_path: self.registry_path.clone(),
            umbilical_path: self.umbilical_path.clone(),
        }
    }

    async fn handle_child(&mut self, mut stream: UnixStream) -> Result<()> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        let req = cell_model::rkyv::check_archived_root::<MitosisRequest>(&buf)
            .map_err(|e| anyhow::anyhow!("Protocol Violation: {}", e))?;

        match req {
            ArchivedMitosisRequest::Spawn { cell_name, config: maybe_config } => { 
                let name_str = cell_name.to_string();
                let source = self.registry_path.join(&name_str);
                
                if !source.exists() {
                    return self.deny(&mut stream, &format!("Cell not found in registry: {}", name_str)).await;
                }

                // 1. Compile
                let binary = match Ribosome::synthesize(&source, &name_str) {
                    Ok(b) => b,
                    Err(e) => return self.deny(&mut stream, &e.to_string()).await,
                };

                // 2. Resolve Configuration
                let final_config = match maybe_config {
                    ArchivedOption::Some(archived_cfg) => {
                        let cfg: CellInitConfig = archived_cfg.deserialize(&mut rkyv::Infallible).unwrap();
                        cfg
                    },
                    ArchivedOption::None => {
                        // Fallback defaults: assumes System scope if not specified
                        let default_sock_path = self.system_socket_dir.join(format!("{}.sock", name_str));
                        CellInitConfig {
                            node_id: rand::random(),
                            cell_name: name_str.clone(),
                            peers: vec![],
                            socket_path: default_sock_path.to_string_lossy().to_string(),
                            organism: "system".to_string(),
                        }
                    }
                };

                // 3. Determine Runtime Directory
                // The cell might live in an organism directory, not the system root.
                // We derive the bind directory from the requested socket path.
                let socket_path = PathBuf::from(&final_config.socket_path);
                let runtime_dir = socket_path.parent().unwrap_or(&self.system_socket_dir);
                
                tokio::fs::create_dir_all(runtime_dir).await?;

                // 4. Inject & Spawn
                match Capsid::spawn(&binary, runtime_dir, &self.umbilical_path, &[], &final_config) {
                    Ok(_) => {
                        let resp = MitosisResponse::Ok { socket_path: final_config.socket_path };
                        self.send_resp(&mut stream, resp).await?;
                    },
                    Err(e) => return self.deny(&mut stream, &e.to_string()).await,
                }
            }
        }
        Ok(())
    }

    async fn deny(&self, stream: &mut UnixStream, reason: &str) -> Result<()> {
        self.send_resp(stream, MitosisResponse::Denied { reason: reason.to_string() }).await
    }

    async fn send_resp(&self, stream: &mut UnixStream, resp: MitosisResponse) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        stream.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }
}