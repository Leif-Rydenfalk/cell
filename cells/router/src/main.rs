// cells/router/src/main.rs
// SPDX-License-Identifier: MIT
// The P2P Switchboard. Defines the protocols (Laser, Satellite, etc).

use anyhow::Result;
use cell_sdk::*;
use cell_core::{channel, VesicleHeader};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use std::path::PathBuf;

// This Router binary defines the Transport Protocol.
// It could be using Lasers, RDMA, or Carrier Pigeons.
// The SDK doesn't know. The SDK just writes to the pipe.

struct RouterState {
    routes: HashMap<u64, String>, 
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    let socket_dir = cell_sdk::resolve_socket_dir();
    let io_dir = socket_dir.join("io");
    std::fs::create_dir_all(&io_dir)?;

    // 1. Discover Outbound Routes
    let mut routes = HashMap::new();
    let neighbors_dir = socket_dir.join("neighbors");
    
    if neighbors_dir.exists() {
        for entry in std::fs::read_dir(neighbors_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            
            let hash = blake3::hash(name.as_bytes());
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&hash.as_bytes()[..8]);
            let id = u64::from_le_bytes(bytes);
            
            routes.insert(id, name.clone());
            println!("[Router] Route: {} -> {}", id, name);
        }
    }
    
    let has_uplink = routes.values().any(|n| n == "uplink" || n == "default");
    let state = Arc::new(RwLock::new(RouterState { routes }));

    // 2. Watch Inbound Pipes (From Local SDKs)
    println!("[Router] Watching {:?}", io_dir);
    
    let mut handled_pipes = std::collections::HashSet::new();

    loop {
        let mut entries = std::fs::read_dir(&io_dir)?;
        while let Some(Ok(entry)) = entries.next() {
            let path = entry.path();
            let fname = entry.file_name().to_string_lossy().to_string();
            
            if fname.ends_with("_in") && !handled_pipes.contains(&fname) {
                let caller_name = fname.trim_end_matches("_in").to_string();
                let pipe_out_name = format!("{}_out", caller_name);
                let pipe_out_path = io_dir.join(&pipe_out_name);
                
                if pipe_out_path.exists() {
                    println!("[Router] Connected: {}", caller_name);
                    handled_pipes.insert(fname.clone());
                    
                    let state = state.clone();
                    let rx_path = path.clone();
                    let tx_path = pipe_out_path.clone();
                    
                    tokio::spawn(async move {
                        if let Err(e) = handle_pipe_pair(rx_path, tx_path, state, has_uplink).await {
                            eprintln!("Pipe error [{}]: {}", caller_name, e);
                        }
                    });
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

async fn handle_pipe_pair(rx_path: PathBuf, tx_path: PathBuf, state: Arc<RwLock<RouterState>>, has_uplink: bool) -> Result<()> {
    let mut rx = OpenOptions::new().read(true).open(&rx_path).await?;
    let mut tx = OpenOptions::new().write(true).open(&tx_path).await?;

    loop {
        // Read Frame
        let mut len_buf = [0u8; 4];
        if rx.read_exact(&mut len_buf).await.is_err() { break; } 
        let len = u32::from_le_bytes(len_buf) as usize;
        
        let mut chan_buf = [0u8; 1];
        rx.read_exact(&mut chan_buf).await?;
        let channel = chan_buf[0];

        if channel == channel::ROUTING {
            let mut header_buf = [0u8; 24];
            rx.read_exact(&mut header_buf).await?;
            
            let target_id = u64::from_le_bytes(header_buf[0..8].try_into()?);
            let mut ttl = header_buf[16];

            let payload_len = len - 1 - 24;
            let mut payload = vec![0u8; payload_len];
            rx.read_exact(&mut payload).await?;

            let guard = state.read().await;
            if let Some(neighbor) = guard.routes.get(&target_id) {
                // HERE IS WHERE THE MAGIC HAPPENS
                // If 'neighbor' is the Laser Controller, this call converts
                // filesystem bytes into Laser Pulses.
                let response = proxy_to_neighbor(neighbor, &payload).await?;
                write_response(&mut tx, channel::APP, &response).await?;
            } else if ttl > 0 && has_uplink {
                ttl -= 1;
                let mut new_header = header_buf;
                new_header[16] = ttl;
                let response = proxy_routed("uplink", &new_header, &payload).await?;
                write_response(&mut tx, channel::APP, &response).await?;
            } else {
                eprintln!("Route not found: {}", target_id);
            }
        } else {
            let mut discard = vec![0u8; len - 1];
            rx.read_exact(&mut discard).await?;
        }
    }
    Ok(())
}

async fn proxy_to_neighbor(neighbor: &str, payload: &[u8]) -> Result<Vec<u8>> {
    let socket_dir = cell_sdk::resolve_socket_dir();
    let link_dir = socket_dir.join("neighbors").join(neighbor);
    
    let mut tx = OpenOptions::new().write(true).open(link_dir.join("tx")).await?;
    let mut rx = OpenOptions::new().read(true).open(link_dir.join("rx")).await?;
    
    let len = 1 + payload.len();
    tx.write_all(&(len as u32).to_le_bytes()).await?;
    tx.write_u8(channel::APP).await?;
    tx.write_all(payload).await?;
    
    read_response(&mut rx).await
}

async fn proxy_routed(neighbor: &str, header: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    let socket_dir = cell_sdk::resolve_socket_dir();
    let link_dir = socket_dir.join("neighbors").join(neighbor);
    
    let mut tx = OpenOptions::new().write(true).open(link_dir.join("tx")).await?;
    let mut rx = OpenOptions::new().read(true).open(link_dir.join("rx")).await?;
    
    let len = 1 + header.len() + payload.len();
    tx.write_all(&(len as u32).to_le_bytes()).await?;
    tx.write_u8(channel::ROUTING).await?;
    tx.write_all(header).await?;
    tx.write_all(payload).await?;
    
    read_response(&mut rx).await
}

async fn write_response(tx: &mut File, chan: u8, payload: &[u8]) -> Result<()> {
    let len = 1 + payload.len();
    tx.write_all(&(len as u32).to_le_bytes()).await?;
    tx.write_u8(chan).await?;
    tx.write_all(payload).await?;
    Ok(())
}

async fn read_response(rx: &mut File) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    rx.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    rx.read_exact(&mut buf).await?;
    if len > 0 { Ok(buf[1..].to_vec()) } else { Ok(vec![]) }
}