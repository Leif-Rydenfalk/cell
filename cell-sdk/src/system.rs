// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use cell_model::protocol::{MitosisRequest, MitosisResponse, MitosisSignal, MitosisControl};
use cell_model::config::CellInitConfig;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, Context, anyhow};
use cell_model::rkyv::Deserialize;
use tokio::sync::OnceCell;
use std::process::{Command, Stdio};
use std::path::PathBuf;
use cell_transport::gap_junction::spawn_with_gap_junction;

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
        
        let daemon_socket = system_dir.join("mitosis.sock");

        if !daemon_socket.exists() {
            return Err(anyhow!("System Daemon not found at {:?}. Is the Hypervisor running?", daemon_socket));
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

        let mut stream = UnixStream::connect(&daemon_socket).await
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
            
            // Only clean runtime state to preserve build artifacts if they exist
            let runtime_dir = target_dir.join("runtime");
            if runtime_dir.exists() {
                let _ = std::fs::remove_dir_all(&runtime_dir);
            }
            std::fs::create_dir_all(&target_dir).unwrap();
            std::fs::create_dir_all(target_dir.join("registry")).unwrap();
            
            let fake_home = target_dir.join("home");
            std::fs::create_dir_all(&fake_home).unwrap();

            // Env Config
            std::env::set_var("CELL_SOCKET_DIR", target_dir.join("runtime/system").to_str().unwrap());
            std::env::set_var("CELL_REGISTRY_DIR", target_dir.join("registry").to_str().unwrap());
            std::env::set_var("CELL_DISABLE_SHM", "0");
            std::env::set_var("CELL_NODE_ID", "100");
            std::env::set_var("HOME", fake_home.to_str().unwrap());

            // Populate Registry (Symlinks)
            if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
                let current = std::path::Path::new(&manifest);
                let registry = target_dir.join("registry");
                let link_cells = |root: PathBuf| {
                    if root.exists() {
                        if let Ok(entries) = std::fs::read_dir(root) {
                            for entry in entries.flatten() {
                                if entry.path().is_dir() && entry.path().join("Cargo.toml").exists() {
                                    let name = entry.file_name();
                                    #[cfg(unix)]
                                    let _ = std::os::unix::fs::symlink(entry.path(), registry.join(name));
                                }
                            }
                        }
                    }
                };
                link_cells(current.join("cells"));
                link_cells(current.join("examples"));
                link_cells(current.parent().unwrap().join("cells")); 
            }

            // --- Binary Optimization Strategy ---
            let mut cmd = if let Some(bin_path) = find_local_binary("hypervisor") {
                // Best Case: Binary already exists from previous build
                Command::new(bin_path)
            } else {
                // Fallback: Build it now.
                // KEY FIX: Removed `--release` and `CARGO_TARGET_DIR` override.
                // This allows reusing the artifacts `cargo test` just compiled (debug profile, main target dir).
                // It prevents recompiling libc/syn/serde from scratch.
                let mut c = Command::new("cargo");
                c.arg("run").arg("-p").arg("hypervisor");
                c
            };
            
            // Setup Gap Junction
            cmd.stdin(Stdio::null());
            cmd.stdout(Stdio::null()); 
            cmd.stderr(Stdio::inherit()); 

            let (_child, mut junction) = spawn_with_gap_junction(cmd).expect("Failed to spawn Hypervisor with Gap Junction");

            // The Progenitor Loop (Blocking wait on Junction)
            let handle = std::thread::spawn(move || {
                loop {
                    let signal = junction.wait_for_signal().expect("Gap Junction severed");
                    
                    match signal {
                        MitosisSignal::RequestIdentity => {
                            let socket_path = target_dir.join("runtime/system/mitosis.sock");
                            let config = CellInitConfig {
                                node_id: 100, // System Node
                                cell_name: "hypervisor".to_string(),
                                peers: vec![],
                                socket_path: socket_path.to_string_lossy().to_string(),
                                organism: "system".to_string(),
                            };
                            
                            junction.send_control(MitosisControl::InjectIdentity(config))
                                .expect("Failed to inject identity");
                        }
                        MitosisSignal::Prophase => { /* Gestating... */ }
                        MitosisSignal::Prometaphase { socket_path: _ } => { /* Binding... */ }
                        MitosisSignal::Cytokinesis => {
                            println!("[System] Hypervisor Cytokinesis complete.");
                            break; // Success
                        }
                        MitosisSignal::Apoptosis { reason } => panic!("[System] Hypervisor Apoptosis: {}", reason),
                        MitosisSignal::Necrosis => panic!("[System] Hypervisor Necrosis."),
                    }
                }
            });

            handle.join().expect("Monitoring thread panicked");

        }).await;
        
        Ok(())
    }
}

fn find_local_binary(name: &str) -> Option<PathBuf> {
    // Attempt to find binary in standard cargo locations
    let current_exe = std::env::current_exe().ok()?;
    let build_dir = current_exe.parent()?.parent()?;
    
    let binary_name = if cfg!(windows) { format!("{}.exe", name) } else { name.to_string() };
    
    // Prioritize debug since we are likely in `cargo test`
    let candidates = [
        build_dir.join("debug").join(&binary_name),
        build_dir.join("release").join(&binary_name),
        current_exe.parent()?.join(&binary_name),
    ];
    
    for c in candidates {
        if c.exists() { return Some(c); }
    }
    None
}