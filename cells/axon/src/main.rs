// cells/axon/src/main.rs
// SPDX-License-Identifier: MIT
// The Network Gateway Cell. Handles QUIC, Pheromones, and WAN routing.

use cell_sdk::resolve_socket_dir;
use cell_axon_lib::{AxonServer, AxonClient, pheromones::PheromoneSystem};
use cell_discovery::LanDiscovery;
use anyhow::{Result, Context};
use tracing::{info, warn, error};
use tokio::net::{UnixListener, UnixStream};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::path::PathBuf;

/// The Proxy Manager tracks discovered remote cells and spawns local proxies.
struct ProxyManager {
    proxies: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
}

impl ProxyManager {
    fn new() -> Self {
        Self {
            proxies: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn maintain_proxies(&self) {
        let socket_dir = resolve_socket_dir();
        tokio::fs::create_dir_all(&socket_dir).await.ok();

        loop {
            // 1. Get all discovered remote signals
            let signals = LanDiscovery::global().all().await;
            
            // 2. Identify unique remote cell names
            let mut remote_cells: HashSet<String> = HashSet::new();
            for sig in signals {
                // If it's not local (check IP logic can be improved), treat as remote.
                // For simplicity, we create proxies for EVERYTHING we see on LAN.
                // If a local socket already exists (created by the real cell), 
                // the proxy bind will fail, which is correct behavior.
                remote_cells.insert(sig.cell_name);
            }

            // 3. Reconcile with active proxies
            let mut proxies = self.proxies.lock().await;
            
            for cell_name in remote_cells {
                if !proxies.contains_key(&cell_name) {
                    // Check if a real local cell owns this socket
                    let path = socket_dir.join(format!("{}.sock", cell_name));
                    if is_socket_active(&path).await {
                        // Real cell is running locally, don't proxy
                        continue;
                    }

                    info!("[Axon] Creating proxy for remote cell '{}'", cell_name);
                    let name_clone = cell_name.clone();
                    let handle = tokio::spawn(async move {
                        if let Err(e) = run_proxy(&name_clone).await {
                            warn!("[Axon] Proxy for '{}' died: {}", name_clone, e);
                        }
                    });
                    proxies.insert(cell_name, handle);
                }
            }
            
            // Note: We don't remove proxies yet to keep connections alive even if discovery flickers.
            
            drop(proxies);
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

async fn is_socket_active(path: &PathBuf) -> bool {
    // Try to connect to see if it's alive
    UnixStream::connect(path).await.is_ok()
}

async fn run_proxy(cell_name: &str) -> Result<()> {
    let socket_dir = resolve_socket_dir();
    let socket_path = socket_dir.join(format!("{}.sock", cell_name));

    // Ensure we clean up stale socket file if bind failed previously
    if socket_path.exists() {
        if !is_socket_active(&socket_path).await {
            tokio::fs::remove_file(&socket_path).await.ok();
        } else {
            // Active socket exists, likely the real cell or another proxy
            return Ok(()); 
        }
    }

    let listener = UnixListener::bind(&socket_path).context("Failed to bind proxy socket")?;
    info!("[Axon] Proxy listening at {:?}", socket_path);

    loop {
        let (mut unix_stream, _) = listener.accept().await?;
        let name_owned = cell_name.to_string();
        
        tokio::spawn(async move {
            if let Err(e) = handle_proxy_connection(&name_owned, unix_stream).await {
                // debug!("[Axon] Proxy connection error: {}", e);
            }
        });
    }
}

async fn handle_proxy_connection(cell_name: &str, mut unix_stream: UnixStream) -> Result<()> {
    // 1. Establish QUIC connection to the remote cell
    // We use AxonClient from the library to do the heavy lifting of discovery + connect
    let quic_conn = match AxonClient::connect(cell_name).await? {
        Some(c) => c,
        None => anyhow::bail!("Could not connect to remote cell '{}'", cell_name),
    };

    // 2. Open bidirectional stream
    let (mut quic_send, mut quic_recv) = quic_conn.open_bi().await?;

    // 3. Pump data bidirectionally
    // We need to split the unix stream to handle simultaneous Read/Write
    let (mut unix_read, mut unix_write) = unix_stream.split();

    // Loop: Unix -> QUIC
    let client_to_server = async {
        tokio::io::copy(&mut unix_read, &mut quic_send).await?;
        quic_send.finish().await?;
        Ok::<_, anyhow::Error>(())
    };

    // Loop: QUIC -> Unix
    let server_to_client = async {
        tokio::io::copy(&mut quic_recv, &mut unix_write).await?;
        Ok::<_, anyhow::Error>(())
    };

    tokio::try_join!(client_to_server, server_to_client)?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Axon] Network Gateway Initializing...");

    let node_id = std::env::var("CELL_NODE_ID")
        .unwrap_or("1".to_string())
        .parse::<u64>()?;
        
    info!("[Axon] Identity: Node {}", node_id);

    // 1. Ignite Pheromone System (UDP Discovery)
    // This allows us to discover other cells on LAN and populate LanDiscovery
    let _pheromones = PheromoneSystem::ignite(node_id).await?;

    // 2. Ignite QUIC Server
    // This allows incoming connections from other Axon Gateways
    let _server = AxonServer::ignite("axon-gateway", node_id).await?;

    // 3. Start the Proxy Manager
    // This creates local Unix sockets for remote cells
    let manager = ProxyManager::new();
    tokio::spawn(async move {
        manager.maintain_proxies().await;
    });

    info!("[Axon] Gateway Active. Bridging Local <-> Global.");
    
    // Keep alive
    std::future::pending::<()>().await;
    
    Ok(())
}