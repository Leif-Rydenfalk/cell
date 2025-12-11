// cells/axon/src/main.rs
// SPDX-License-Identifier: MIT
// The Network Gateway Cell. Handles QUIC, Pheromones, and WAN routing.

use cell_sdk::*;
use cell_sdk::resolve_socket_dir;
use cell_axon_lib::{AxonServer, AxonClient, pheromones::PheromoneSystem};
use cell_model::bridge::{BridgeRequest, BridgeResponse};
use anyhow::{Result, Context};
use tracing::{info, warn, error};
use tokio::net::{UnixListener, UnixStream};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::path::PathBuf;

/// The Proxy Manager creates on-demand tunnels
struct ProxyManager {
    proxies: Arc<Mutex<HashMap<String, String>>>, // Map target -> socket_path
}

impl ProxyManager {
    fn new() -> Self {
        Self {
            proxies: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn ensure_proxy(&self, target: &str) -> Result<String> {
        let mut proxies = self.proxies.lock().await;
        
        if let Some(path) = proxies.get(target) {
            // Verify it's still alive
            if UnixStream::connect(path).await.is_ok() {
                return Ok(path.clone());
            }
            // If dead, cleanup and recreate
            proxies.remove(target);
        }

        let socket_dir = resolve_socket_dir();
        tokio::fs::create_dir_all(&socket_dir).await.ok();
        
        // Generate a deterministic proxy path: /tmp/cell/axon_proxy_<sanitized_target>.sock
        let safe_target = target.replace([':', '/'], "_");
        let path = socket_dir.join(format!("axon_proxy_{}.sock", safe_target));
        let path_str = path.to_string_lossy().to_string();

        // Cleanup stale file
        if path.exists() {
            tokio::fs::remove_file(&path).await.ok();
        }

        // Bind listener
        let listener = UnixListener::bind(&path).context("Failed to bind proxy socket")?;
        
        let target_clone = target.to_string();
        
        // Spawn the proxy pump
        tokio::spawn(async move {
            info!("[Axon] Spawning proxy for '{}' at {:?}", target_clone, path);
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let target = target_clone.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_proxy_connection(&target, stream).await {
                                // debug!("[Axon] Proxy tunnel error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("[Axon] Proxy listener error: {}", e);
                        break;
                    }
                }
            }
        });

        proxies.insert(target.to_string(), path_str.clone());
        Ok(path_str)
    }
}

async fn handle_proxy_connection(target: &str, mut unix_stream: UnixStream) -> Result<()> {
    // 1. Resolve Target via Discovery / DHT
    // If target is simple name "ledger", use Pheromones/DHT.
    // If target is "192.168.1.50:9000", use direct connect.
    
    // For this implementation, we assume `AxonClient::connect` handles the discovery logic
    // internally using Pheromones for names or parsing IPs.
    let quic_conn = match AxonClient::connect(target).await? {
        Some(c) => c,
        None => anyhow::bail!("Could not connect to remote target '{}'", target),
    };

    let (mut quic_send, mut quic_recv) = quic_conn.open_bi().await?;
    let (mut unix_read, mut unix_write) = unix_stream.split();

    let client_to_server = async {
        tokio::io::copy(&mut unix_read, &mut quic_send).await?;
        quic_send.finish().await?;
        Ok::<_, anyhow::Error>(())
    };

    let server_to_client = async {
        tokio::io::copy(&mut quic_recv, &mut unix_write).await?;
        Ok::<_, anyhow::Error>(())
    };

    tokio::try_join!(client_to_server, server_to_client)?;
    Ok(())
}

struct AxonService {
    proxy_manager: Arc<ProxyManager>,
}

#[handler]
impl AxonService {
    async fn mount(&self, target: String) -> BridgeResponse {
        match self.proxy_manager.ensure_proxy(&target).await {
            Ok(path) => BridgeResponse::Mounted { socket_path: path },
            Err(e) => {
                error!("[Axon] Mount failed: {}", e);
                BridgeResponse::Error { message: e.to_string() }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    let node_id = std::env::var("CELL_NODE_ID")
        .unwrap_or("1".to_string())
        .parse::<u64>()?;
    
    info!("[Axon] Network Gateway Initializing (Node {})...", node_id);

    // 1. Infrastructure
    let _pheromones = PheromoneSystem::ignite(node_id).await?;
    let _server = AxonServer::ignite("axon", node_id).await?; // Advertising as "axon" gateway

    // 2. Proxy Manager
    let proxy_manager = Arc::new(ProxyManager::new());

    // 3. Serve the Bridge Protocol (Local RPC)
    let service = AxonService { proxy_manager };
    
    // This creates ~/.cell/run/axon.sock which SDKs look for
    service.serve("axon").await
}