// cells/mycelium/src/main.rs
// SPDX-License-Identifier: MIT
// The Supervisor

// ... (Imports same as before) ...
use anyhow::{Result, Context, anyhow};
use std::os::unix::net::{UnixListener, UnixStream};
use std::io::{Read, Write};
use tracing::{info, warn, error};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;
use cell_model::protocol::{MitosisSignal, MitosisControl};
use cell_model::config::CellInitConfig;
use cell_transport::gap_junction::spawn_with_gap_junction;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub enum ResolverRequest { EnsureRunning { cell_name: String } }
#[derive(Serialize, Deserialize, Debug)]
pub enum ResolverResponse { Ok { socket_path: String }, Error { message: String } }

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("CELL_SOCKET_DIR").is_ok() { std::env::remove_var("CELL_SOCKET_DIR"); }
    std::env::remove_var("CELL_NODE_ID");
    std::env::remove_var("CELL_ORGANISM");

    tracing_subscriber::fmt().init();
    info!("--- MYCELIUM ONLINE ---");

    let home = dirs::home_dir().expect("No HOME");
    let system_dir = home.join(".cell/runtime/system");
    tokio::fs::create_dir_all(&system_dir).await?;
    
    let mycelium_sock = system_dir.join("mycelium.sock");
    if mycelium_sock.exists() { tokio::fs::remove_file(&mycelium_sock).await.ok(); }

    ensure_hypervisor_running().await?;

    let listener = UnixListener::bind(&mycelium_sock)?;
    info!("[Mycelium] Listening...");

    let listener_handle = tokio::task::spawn_blocking(move || {
        loop {
            if let Ok((mut stream, _)) = listener.accept() {
                tokio::spawn(async move {
                    let _ = handle_client(&mut stream).await;
                });
            }
        }
    });

    let health_handle = tokio::spawn(async move {
        loop { tokio::time::sleep(Duration::from_secs(60)).await; }
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
            // ALWAYS call ensure_cell, which now calls Hypervisor->Spawn
            // Hypervisor handles idempotency and hot-swapping.
            match ensure_cell(&cell_name).await {
                Ok(path) => ResolverResponse::Ok { socket_path: path },
                Err(e) => ResolverResponse::Error { message: e.to_string() },
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
    // We do NOT check for socket existence here anymore.
    // We actively request spawn to ensure version compliance.
    let path = cell_sdk::System::spawn(name, None).await
        .context("Failed to spawn/update cell via Hypervisor")?;
    Ok(path)
}

// ... (ensure_hypervisor_running and find_binary remain same) ...
async fn ensure_hypervisor_running() -> Result<()> {
    let home = dirs::home_dir().unwrap();
    let system_dir = home.join(".cell/runtime/system");
    let hv_sock = system_dir.join("mitosis.sock");
    
    if hv_sock.exists() { return Ok(()); }

    info!("[Mycelium] Booting Hypervisor...");
    let current_exe = std::env::current_exe()?;
    let bin_dir = current_exe.parent().context("No parent dir")?;
    let hypervisor_bin = bin_dir.join("hypervisor"); // Assume built alongside mycelium

    let mut cmd = std::process::Command::new(&hypervisor_bin);
    cmd.env("CELL_SOCKET_DIR", system_dir.to_str().unwrap());
    cmd.env("CELL_NODE_ID", "0");
    cmd.env("CELL_ORGANISM", "system");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::inherit()); 

    let (_child, mut junction) = spawn_with_gap_junction(cmd)?;

    loop {
        match junction.wait_for_signal()? {
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
            MitosisSignal::Cytokinesis => break,
            _ => {}
        }
    }
    Ok(())
}