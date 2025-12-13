// cells/mycelium/src/main.rs
// SPDX-License-Identifier: MIT
// The Supervisor: Auto-spawns, Heals, and Scales the Mesh.

use anyhow::{Result, Context};
use cell_sdk::cell_remote;
use cell_model::config::CellInitConfig;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::os::unix::net::{UnixListener, UnixStream};
use std::io::{Read, Write};
use tracing::{info, warn, error};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

// Mycelium needs to talk to Hypervisor to actually spawn processes
cell_remote!(Hypervisor = "hypervisor");

// Use the protocol defined in cell-build
#[derive(Serialize, Deserialize, Debug)]
pub enum ResolverRequest {
    EnsureRunning { cell_name: String },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ResolverResponse {
    Ok { socket_path: String },
    Error { message: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("╔══════════════════════════════════════════════════════════╗");
    info!("║           MYCELIUM SUPERVISOR ONLINE                     ║");
    info!("║   The Root of the Mesh.                                  ║");
    info!("╚══════════════════════════════════════════════════════════╝");

    // 1. Setup Environment
    let home = dirs::home_dir().expect("No HOME");
    let system_dir = home.join(".cell/runtime/system");
    tokio::fs::create_dir_all(&system_dir).await?;
    
    let mycelium_sock = system_dir.join("mycelium.sock");
    if mycelium_sock.exists() { tokio::fs::remove_file(&mycelium_sock).await.ok(); }

    // 2. Ensure Hypervisor is running (Mycelium's right hand)
    ensure_hypervisor_running().await?;

    // 3. Start Build Listener (Blocking IO in a thread or async adapter)
    let listener = UnixListener::bind(&mycelium_sock)?;
    info!("[Mycelium] Listening for build requests at {:?}", mycelium_sock);

    // We use a simple loop + tokio spawn for the resolver requests
    let listener_handle = tokio::task::spawn_blocking(move || {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Handle client in a thread/async task
                    // We need a way to call async code (hypervisor) from this blocking stream
                    // We spawn a tokio task to handle it.
                    let runtime = tokio::runtime::Handle::current();
                    runtime.spawn(async move {
                        if let Err(e) = handle_client(&mut stream).await {
                            error!("[Mycelium] Client error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("[Mycelium] Accept error: {}", e);
                    break;
                }
            }
        }
    });

    // 4. Start standard monitoring (Pheromones, Health)
    // (Stubbed for now to focus on Resolver)
    let health_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            // info!("[Mycelium] Heartbeat...");
        }
    });

    let _ = tokio::join!(health_handle, listener_handle);
    Ok(())
}

async fn handle_client(stream: &mut UnixStream) -> Result<()> {
    // Read Request
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;

    let req: ResolverRequest = serde_json::from_slice(&buf)?;

    let resp = match req {
        ResolverRequest::EnsureRunning { cell_name } => {
            info!("[Mycelium] Request to ensure '{}' is active", cell_name);
            match ensure_cell(&cell_name).await {
                Ok(path) => ResolverResponse::Ok { socket_path: path },
                Err(e) => {
                    warn!("[Mycelium] Failed to spawn {}: {}", cell_name, e);
                    ResolverResponse::Error { message: e.to_string() }
                },
            }
        }
    };

    let resp_bytes = serde_json::to_vec(&resp)?;
    let len_out = resp_bytes.len() as u32;
    stream.write_all(&len_out.to_le_bytes())?;
    stream.write_all(&resp_bytes)?;
    Ok(())
}

async fn ensure_cell(name: &str) -> Result<String> {
    let home = dirs::home_dir().unwrap();
    let socket_path = home.join(".cell/runtime/system").join(format!("{}.sock", name));
    
    // 1. Check if already running (File existence check)
    if socket_path.exists() {
        return Ok(socket_path.to_string_lossy().to_string());
    }

    // 2. Not running. Spawn it via Hypervisor.
    info!("[Mycelium] '{}' missing. Spawning...", name);
    
    // Connect to Hypervisor
    let mut hypervisor = Hypervisor::Client::connect().await
        .context("Failed to reach Hypervisor")?;

    // Hypervisor returns Result<String, CellError> (socket path)
    let path_res = hypervisor.spawn(name.to_string(), None).await?;
    let path = match path_res {
        Ok(p) => p,
        Err(e) => anyhow::bail!("Spawn denied by Hypervisor: {:?}", e),
    };
    
    Ok(path)
}

async fn ensure_hypervisor_running() -> Result<()> {
    // Check if Hypervisor socket exists
    let home = dirs::home_dir().unwrap();
    let hv_sock = home.join(".cell/runtime/system/mitosis.sock");
    
    if hv_sock.exists() {
        return Ok(());
    }

    info!("[Mycelium] Hypervisor missing. Igniting local cluster...");
    // Use SDK to ignite (this spawns the hypervisor process)
    cell_sdk::System::ignite_local_cluster().await?;
    
    // Wait a bit
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    Ok(())
}