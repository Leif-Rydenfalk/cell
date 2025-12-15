// cells/router/src/main.rs
// SPDX-License-Identifier: MIT
// The Opt-In Network Switchboard. Pure P2P.

use anyhow::Result;
use cell_sdk::*;
use cell_core::{channel, VesicleHeader};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

// Defines topology in memory
struct RouterState {
    // Hash -> Neighbor Name
    routes: HashMap<u64, String>, 
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    // 1. Discover Neighbors from Filesystem
    let mut routes = HashMap::new();
    let socket_dir = cell_sdk::resolve_socket_dir();
    let neighbors_dir = socket_dir.join("neighbors");
    
    if neighbors_dir.exists() {
        for entry in std::fs::read_dir(neighbors_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            
            // Calculate Hash
            let hash = blake3::hash(name.as_bytes());
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&hash.as_bytes()[..8]);
            let id = u64::from_le_bytes(bytes);
            
            routes.insert(id, name.clone());
            println!("[Router] Route added: {} -> {}", id, name);
        }
    }

    // Also add "uplink" or "default" to recursive routes if they exist?
    // For P2P recursion: If we have an "uplink" neighbor, we use it as default gateway.
    let has_uplink = routes.values().any(|n| n == "uplink" || n == "default");

    let state = Arc::new(RwLock::new(RouterState { routes }));
    
    // 2. Bind Socket manually (We are a low-level infra cell)
    let my_socket = socket_dir.join("cell.sock");
    if my_socket.exists() { std::fs::remove_file(&my_socket)?; }
    let listener = UnixListener::bind(&my_socket)?;
    
    println!("[Router] Listening on {:?}", my_socket);

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, state, has_uplink).await {
                eprintln!("Connection error: {}", e);
            }
        });
    }
}

async fn handle_connection(mut stream: UnixStream, state: Arc<RwLock<RouterState>>, has_uplink: bool) -> Result<()> {
    // Protocol Loop
    loop {
        // 1. Read Frame Length
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() { break; } // EOF
        let len = u32::from_le_bytes(len_buf) as usize;
        
        // 2. Read Channel
        let mut chan_buf = [0u8; 1];
        stream.read_exact(&mut chan_buf).await?;
        let channel = chan_buf[0];

        // 3. Routing Logic
        if channel == channel::ROUTING {
            // Layout: [Header (24)][Payload...]
            let mut header_buf = [0u8; 24];
            stream.read_exact(&mut header_buf).await?;
            
            // Parse Header
            let target_id = u64::from_le_bytes(header_buf[0..8].try_into()?);
            let _source_id = u64::from_le_bytes(header_buf[8..16].try_into()?);
            let mut ttl = header_buf[16];
            let _pad = &header_buf[17..];

            // Payload
            let payload_len = len - 1 - 24; // Total - Chan - Header
            let mut payload = vec![0u8; payload_len];
            stream.read_exact(&mut payload).await?;

            // Lookup
            let guard = state.read().await;
            if let Some(neighbor) = guard.routes.get(&target_id) {
                // FOUND LOCAL
                forward_to_neighbor(neighbor, &payload).await?;
                // We send back response? 
                // This naive implementation is one-way or assumes req-resp on stream
                // Real router needs to bridge streams.
                // For MVP: We assume the target creates a new connection back? 
                // No, RPC expects response on same stream.
                // We need to bridge the connection.
                
                // Correct P2P Router Logic:
                // We shouldn't have read the payload into memory if we want to stream.
                // But for vesicle message passing, memory is fine.
                // Problem: How to get response back to `stream`?
                // `forward_to_neighbor` needs to write to neighbor AND read response AND write back to `stream`.
                
                // Let's implement simple request-response proxy
                let response = proxy_request(neighbor, &payload).await?;
                
                // Write back to caller
                let resp_total_len = 1 + response.len();
                stream.write_all(&(resp_total_len as u32).to_le_bytes()).await?;
                stream.write_u8(channel::APP).await?; // Return on APP channel (unwrapped)
                stream.write_all(&response).await?;
                
            } else if ttl > 0 && has_uplink {
                // RECURSIVE ROUTING
                ttl -= 1;
                // Re-pack header
                let mut new_header = header_buf;
                new_header[16] = ttl;
                
                let response = proxy_routed_request("uplink", &new_header, &payload).await?;
                
                let resp_total_len = 1 + response.len();
                stream.write_all(&(resp_total_len as u32).to_le_bytes()).await?;
                stream.write_u8(channel::APP).await?;
                stream.write_all(&response).await?;
                
            } else {
                eprintln!("Route not found for {}", target_id);
            }
        } else {
            // Consume remaining bytes to keep stream sync (or drop)
            let mut discard = vec![0u8; len - 1];
            stream.read_exact(&mut discard).await?;
        }
    }
    Ok(())
}

async fn proxy_request(neighbor: &str, payload: &[u8]) -> Result<Vec<u8>> {
    // Connect to neighbor socket
    let socket_dir = cell_sdk::resolve_socket_dir();
    let sock_path = socket_dir.join("neighbors").join(neighbor);
    
    let mut conn = UnixStream::connect(sock_path).await?;
    
    // Send standard APP frame
    let len = 1 + payload.len();
    conn.write_all(&(len as u32).to_le_bytes()).await?;
    conn.write_u8(channel::APP).await?;
    conn.write_all(payload).await?;
    
    // Read Response
    read_frame(&mut conn).await
}

async fn proxy_routed_request(neighbor: &str, header: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    let socket_dir = cell_sdk::resolve_socket_dir();
    let sock_path = socket_dir.join("neighbors").join(neighbor);
    let mut conn = UnixStream::connect(sock_path).await?;
    
    // Send ROUTING frame
    let len = 1 + header.len() + payload.len();
    conn.write_all(&(len as u32).to_le_bytes()).await?;
    conn.write_u8(channel::ROUTING).await?;
    conn.write_all(header).await?;
    conn.write_all(payload).await?;
    
    // Read Response
    read_frame(&mut conn).await
}

async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    // Strip channel
    if len > 0 { Ok(buf[1..].to_vec()) } else { Ok(vec![]) }
}