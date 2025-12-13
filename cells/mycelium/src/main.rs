// cells/mycelium/src/main.rs
// SPDX-License-Identifier: MIT
// The Supervisor: Auto-spawns, Heals, and Scales the Mesh.

use anyhow::{Result, Context, anyhow};
use std::os::unix::net::{UnixListener, UnixStream};
use std::io::{Read, Write};
use tracing::{info, warn, error};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;
use std::path::PathBuf;

// Imports for Handshake
use cell_model::protocol::{MitosisSignal, MitosisControl};
use cell_model::config::CellInitConfig;
use cell_transport::gap_junction::spawn_with_gap_junction;

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
    // IMPORTANT: Spawns hypervisor in the SYSTEM scope via Gap Junction
    ensure_hypervisor_running().await?;

    // 3. Start Build Listener (Blocking IO in a thread or async adapter)
    let listener = UnixListener::bind(&mycelium_sock)?;
    info!("[Mycelium] Listening for build requests at {:?}", mycelium_sock);

    // We use a simple loop + tokio spawn for the resolver requests
    let listener_handle = tokio::task::spawn_blocking(move || {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let runtime = tokio::runtime::Handle::current();
                    runtime.spawn(async move {
                        if let Err(e) = handle_client(&mut stream).await {
                            error!("[Mycelium] Client error: {:?}", e);
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
    let health_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    let _ = tokio::join!(health_handle, listener_handle);
    Ok(())
}

async fn handle_client(stream: &mut UnixStream) -> Result<()> {
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
                    // Use Debug formatting {:#} or {:?} to print the full error chain
                    warn!("[Mycelium] Failed to spawn {}: {:#}", cell_name, e);
                    ResolverResponse::Error { message: format!("{:#}", e) }
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
    
    if socket_path.exists() {
        return Ok(socket_path.to_string_lossy().to_string());
    }

    info!("[Mycelium] '{}' missing. Spawning via Hypervisor...", name);
    let path = cell_sdk::System::spawn(name, None).await
        .context("Failed to spawn cell via Hypervisor")?;
    
    Ok(path)
}

async fn ensure_hypervisor_running() -> Result<()> {
    let home = dirs::home_dir().unwrap();
    let system_dir = home.join(".cell/runtime/system");
    let hv_sock = system_dir.join("mitosis.sock");
    
    if hv_sock.exists() {
        // Ping check? For now assume existence implies running
        return Ok(());
    }

    info!("[Mycelium] Hypervisor missing. Igniting system kernel...");

    // 1. Locate Binary
    // We try to find the binary relative to our current executable
    // (assuming we are in target/release/ or similar)
    let current_exe = std::env::current_exe()?;
    let bin_dir = current_exe.parent().context("No parent dir")?;
    let hypervisor_bin = bin_dir.join("hypervisor");

    let final_bin_path = if hypervisor_bin.exists() {
        hypervisor_bin
    } else {
        // Fallback: Build it if missing
        info!("[Mycelium] Hypervisor binary not found at {:?}. Compiling...", hypervisor_bin);
        let status = std::process::Command::new("cargo")
            .args(&["build", "--release", "-p", "hypervisor"])
            .status()?;
        
        if !status.success() {
            anyhow::bail!("Failed to compile hypervisor");
        }
        
        // Re-locate after build
        find_binary("hypervisor").ok_or_else(|| anyhow!("Could not locate hypervisor binary after build"))?
    };

    // 2. Prepare Command
    let mut cmd = std::process::Command::new(&final_bin_path);
    cmd.env("CELL_SOCKET_DIR", system_dir.to_str().unwrap());
    cmd.env("CELL_NODE_ID", "0");
    cmd.env("CELL_ORGANISM", "system");
    
    // Gap Junction setup
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null()); // Daemonize
    cmd.stderr(std::process::Stdio::inherit()); // Keep logs

    // 3. Spawn with Bridge
    let (_child, mut junction) = spawn_with_gap_junction(cmd)?;

    // 4. Perform Handshake (Progenitor Role)
    loop {
        let signal = junction.wait_for_signal()?;
        match signal {
            MitosisSignal::RequestIdentity => {
                let config = CellInitConfig {
                    node_id: 0,
                    cell_name: "hypervisor".to_string(),
                    peers: vec![],
                    socket_path: hv_sock.to_string_lossy().to_string(),
                    organism: "system".to_string(),
                };
                junction.send_control(MitosisControl::InjectIdentity(config))?;
            }
            MitosisSignal::Prophase => { info!("[Mycelium] Hypervisor gestation..."); }
            MitosisSignal::Prometaphase { socket_path } => { info!("[Mycelium] Hypervisor bound at {}", socket_path); }
            MitosisSignal::Cytokinesis => {
                info!("[Mycelium] Hypervisor detached and online.");
                break;
            }
            MitosisSignal::Apoptosis { reason } => anyhow::bail!("Hypervisor died: {}", reason),
            MitosisSignal::Necrosis => anyhow::bail!("Hypervisor panicked"),
        }
    }

    Ok(())
}

fn find_binary(name: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let candidates = [
        cwd.join("target/release").join(name),
        cwd.join("target/debug").join(name),
        cwd.join(format!("cells/{}/target/release/{}", name, name)),
    ];
    for c in candidates {
        if c.exists() { return Some(c); }
    }
    None
}