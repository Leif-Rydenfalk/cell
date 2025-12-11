// cells/axon/src/main.rs
// SPDX-License-Identifier: MIT
// The Network Gateway Cell. Handles QUIC, Pheromones, and WAN routing.

use cell_sdk::*;
use cell_sdk::resolve_socket_dir;
use cell_axon_lib::{AxonServer, AxonClient, pheromones::PheromoneSystem};
use cell_model::bridge::{BridgeRequest, BridgeResponse};
use cell_model::protocol::{SHM_UPGRADE_REQUEST, SHM_UPGRADE_ACK};
use anyhow::{Result, Context};
use tracing::{info, warn, error};
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::collections::{HashMap};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::path::PathBuf;
use std::os::unix::io::AsRawFd;

// Import SHM internals from the SDK transport layer
use cell_transport::shm::{RingBuffer};
use cell_transport::membrane::{get_shm_auth_token, send_fds};

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
            proxies.remove(target);
        }

        let socket_dir = resolve_socket_dir();
        tokio::fs::create_dir_all(&socket_dir).await.ok();
        
        // Generate a deterministic proxy path: /tmp/cell/axon_proxy_<target>.sock
        let safe_target = target.replace([':', '/'], "_");
        let path = socket_dir.join(format!("axon_proxy_{}.sock", safe_target));
        let path_str = path.to_string_lossy().to_string();

        if path.exists() {
            tokio::fs::remove_file(&path).await.ok();
        }

        let listener = UnixListener::bind(&path).context("Failed to bind proxy socket")?;
        
        let target_clone = target.to_string();
        
        tokio::spawn(async move {
            info!("[Axon] Spawning proxy for '{}' at {:?}", target_clone, path);
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let target = target_clone.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_smart_proxy_connection(&target, stream).await {
                                // Log debug only to avoid spam on disconnects
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

/// Handles a connection to the proxy socket.
/// Supports "Smart Bridging": If client requests SHM, we bridge SHM <-> QUIC directly.
/// Otherwise, we bridge Unix <-> QUIC.
async fn handle_smart_proxy_connection(target: &str, mut unix_stream: UnixStream) -> Result<()> {
    // 1. Establish QUIC connection to the remote cell
    let quic_conn = match AxonClient::connect(target).await? {
        Some(c) => c,
        None => anyhow::bail!("Could not connect to remote target '{}'", target),
    };

    let (mut quic_send, mut quic_recv) = quic_conn.open_bi().await?;

    // 2. Peek at incoming bytes to check for SHM upgrade request
    // Synapse sends: [Len u32][SHM_UPGRADE_REQUEST]
    // We read the length first.
    let mut len_buf = [0u8; 4];
    let n = unix_stream.read_exact(&mut len_buf).await?;
    if n == 0 { return Ok(()); }
    
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    unix_stream.read_exact(&mut body).await?;

    // Check if it's an SHM Upgrade Request. 
    // Format on wire: [Channel u8][SHM_UPGRADE_REQUEST bytes...]
    // The transport::UnixConnection adds the channel byte. Synapse.try_upgrade sends raw request though?
    // Let's check `try_upgrade_to_shm` in synapse.rs:
    // It sends: frame = [0x00, SHM_UPGRADE_REQUEST...]
    // So yes, body[0] is channel 0, body[1..] is request.
    
    let is_shm_request = body.len() > 1 && body[1..] == SHM_UPGRADE_REQUEST[..];

    if is_shm_request {
        // --- SHM BRIDGE PATH ---
        // We act as the Server in the SHM handshake.
        
        let cred = unix_stream.peer_cred()?;
        let my_uid = nix::unistd::getuid().as_raw();
        if cred.uid() != my_uid { anyhow::bail!("UID mismatch during proxy upgrade"); }

        // Challenge-Response
        let challenge: [u8; 32] = rand::random();
        unix_stream.write_all(&challenge).await?;
        
        let mut response = [0u8; 32];
        unix_stream.read_exact(&mut response).await?;
        
        let auth_token = get_shm_auth_token()?;
        let expected = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
        
        if response != expected.as_bytes()[..32] {
            anyhow::bail!("Auth failed during proxy upgrade");
        }

        // Create RingBuffers for this session
        // Note: In proxy mode, we create new rings for every connection.
        let rx_name = format!("axon_proxy_{}_rx", rand::random::<u32>());
        let tx_name = format!("axon_proxy_{}_tx", rand::random::<u32>());
        
        let (rx_ring, rx_fd) = RingBuffer::create(&rx_name)?;
        let (tx_ring, tx_fd) = RingBuffer::create(&tx_name)?;

        // Send Ack + FDs
        unix_stream.write_all(&(SHM_UPGRADE_ACK.len() as u32).to_le_bytes()).await?;
        unix_stream.write_all(SHM_UPGRADE_ACK).await?;

        send_fds(unix_stream.as_raw_fd(), &[rx_fd, tx_fd])?;

        // Pump Data: SHM <-> QUIC
        // rx_ring: Client writes here, Axon reads.
        // tx_ring: Client reads here, Axon writes.
        
        let shm_reader = rx_ring;
        let shm_writer = tx_ring;

        // Task A: Read SHM -> Write QUIC
        let shm_to_quic = async {
            loop {
                if let Ok(Some(msg)) = shm_reader.try_read_raw() {
                    let data = msg.get_bytes();
                    let channel = msg.channel();
                    let len = data.len();
                    
                    // Wire format for QUIC to look like Unix stream to remote:
                    // [TotalLen u32][Channel u8][Data...]
                    let total_len = (1 + len) as u32;
                    
                    quic_send.write_all(&total_len.to_le_bytes()).await?;
                    quic_send.write_u8(channel).await?;
                    quic_send.write_all(data).await?;
                    
                    // Release slot
                    drop(msg);
                } else {
                    // Spin/Sleep
                    tokio::time::sleep(std::time::Duration::from_nanos(500)).await;
                }
            }
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        };

        // Task B: Read QUIC -> Write SHM
        let quic_to_shm = async {
            let mut len_buf = [0u8; 4];
            loop {
                quic_recv.read_exact(&mut len_buf).await?;
                let total_len = u32::from_le_bytes(len_buf) as usize;
                if total_len == 0 { break; } 

                let channel = quic_recv.read_u8().await?;
                let data_len = total_len - 1;
                
                // Write to SHM
                let mut slot = shm_writer.wait_for_slot(data_len).await;
                
                // Read directly into temporary buffer then write to slot
                // (Optimally we'd read into slot but wait_for_slot gives us write access via &mut [u8] implicitly)
                let mut buf = vec![0u8; data_len];
                quic_recv.read_exact(&mut buf).await?;
                
                slot.write(&buf, channel);
                slot.commit(data_len);
            }
            Ok::<_, anyhow::Error>(())
        };

        // Race them
        tokio::select! {
            res = shm_to_quic => res?,
            res = quic_to_shm => res?,
        }

    } else {
        // --- UNIX FALLBACK PATH ---
        // We already read the first packet (len + body). We must forward it.
        
        quic_send.write_all(&len_buf).await?;
        quic_send.write_all(&body).await?;

        // Standard pump for the rest of the stream
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
    }

    Ok(())
}

struct AxonService {
    proxy_manager: Arc<ProxyManager>,
}

#[handler]
impl AxonService {
    // Implements the Bridge Protocol
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

    // 1. Infrastructure (Discovery + QUIC Listener)
    let _pheromones = PheromoneSystem::ignite(node_id).await?;
    let _server = AxonServer::ignite("axon", node_id).await?; 

    // 2. Proxy Manager
    let proxy_manager = Arc::new(ProxyManager::new());

    // 3. Serve the Axon Bridge Service
    let service = AxonService { proxy_manager };
    
    // This creates ~/.cell/run/axon.sock
    service.serve("axon").await
}