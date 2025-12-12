// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use cell_model::config::CellInitConfig;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, Context, anyhow};
use cell_model::rkyv::Deserialize;
use tokio::sync::OnceCell;

// Tracks if we have already ignited a local cluster in this process
static CLUSTER_INIT: OnceCell<()> = OnceCell::const_new();

/// Client interface for the Cell System Daemon (Hypervisor).
pub struct System;

impl System {
    /// Request the Daemon to spawn a new Cell.
    pub async fn spawn(cell_name: &str, mut config: Option<CellInitConfig>) -> Result<String> {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        
        let system_dir = if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
            std::path::PathBuf::from(p)
        } else {
            home.join(".cell/runtime/system")
        };
        
        let umbilical = system_dir.join("mitosis.sock");

        if !umbilical.exists() {
            return Err(anyhow!("System Daemon not found at {:?}. Is the Hypervisor running?", umbilical));
        }

        if config.is_none() {
            let org = std::env::var("CELL_ORGANISM").unwrap_or_else(|_| "system".to_string());
            let socket_dir = resolve_socket_dir(); 
            let socket_path = socket_dir.join(format!("{}.sock", cell_name));
            
            config = Some(CellInitConfig {
                node_id: rand::random(),
                cell_name: cell_name.to_string(),
                peers: vec![],
                socket_path: socket_path.to_string_lossy().to_string(),
                organism: org,
            });
        }

        let mut stream = UnixStream::connect(&umbilical).await
            .context("Failed to connect to Hypervisor")?;

        let req = MitosisRequest::Spawn {
            cell_name: cell_name.to_string(),
            config,
        };

        let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
        stream.write_all(&(req_bytes.len() as u32).to_le_bytes()).await?;
        stream.write_all(&req_bytes).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        
        let mut resp_buf = vec![0u8; len];
        stream.read_exact(&mut resp_buf).await?;

        let archived = cell_model::rkyv::check_archived_root::<MitosisResponse>(&resp_buf)
            .map_err(|e| anyhow!("Invalid system response: {:?}", e))?;
            
        let resp: MitosisResponse = archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap();

        match resp {
            MitosisResponse::Ok { socket_path } => Ok(socket_path),
            MitosisResponse::Denied { reason } => Err(anyhow!("Spawn denied: {}", reason)),
        }
    }

    /// Bootstraps a complete, isolated local Cell environment by running the Hypervisor cell.
    pub async fn ignite_local_cluster() -> Result<()> {
        CLUSTER_INIT.get_or_init(|| async {
            let mut target_dir = std::env::current_dir().unwrap();
            target_dir.push("target");
            target_dir.push("cell-cluster");
            
            if target_dir.exists() {
                let _ = std::fs::remove_dir_all(&target_dir);
            }
            std::fs::create_dir_all(&target_dir).unwrap();

            // Config for the Daemon Process
            std::env::set_var("CELL_SOCKET_DIR", target_dir.join("runtime/system").to_str().unwrap());
            std::env::set_var("CELL_REGISTRY_DIR", target_dir.join("registry").to_str().unwrap());
            std::env::set_var("CELL_DISABLE_SHM", "0");
            std::env::set_var("CELL_NODE_ID", "100");
            
            let fake_home = target_dir.join("home");
            std::fs::create_dir_all(&fake_home).unwrap();
            std::env::set_var("HOME", fake_home.to_str().unwrap());

            // Populate Registry
            if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
                let current = std::path::Path::new(&manifest);
                let potential_roots = [
                    current.to_path_buf(),
                    current.parent().unwrap().to_path_buf(),
                    current.join("../"),
                    current.join("../../"),
                ];
                
                let registry = target_dir.join("registry");
                std::fs::create_dir_all(&registry).unwrap();

                for root in potential_roots {
                    let candidates = [root.join("cells"), root.join("examples")];
                    for candidate in candidates {
                        if candidate.exists() {
                            if let Ok(entries) = std::fs::read_dir(candidate) {
                                for entry in entries.flatten() {
                                    if entry.path().is_dir() && entry.path().join("Cargo.toml").exists() {
                                        let name = entry.file_name();
                                        #[cfg(unix)]
                                        let _ = std::os::unix::fs::symlink(entry.path(), registry.join(name));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Launch Hypervisor Daemon via Cargo
            // This decouples the SDK from the daemon implementation.
            let mut cmd = std::process::Command::new("cargo");
            cmd.arg("run").arg("--release").arg("-p").arg("hypervisor");
            
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::inherit());

            // Detach and run
            std::thread::spawn(move || { let _ = cmd.spawn(); });

            // Wait for system to be ready.
            // The Hypervisor manages spawning core services (builder, nucleus, axon).
            // Once the umbilical socket exists, the Hypervisor is active.
            // Requests sent to it will be queued until it finishes bootstrapping.
            let umbilical = target_dir.join("runtime/system/mitosis.sock");
            for _ in 0..100 {
                if umbilical.exists() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            
            // Give it a moment to start bootstrapping children inside the daemon
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        }).await;
        
        Ok(())
    }
}