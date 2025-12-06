// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use crate::transport::{UnixListenerAdapter, UnixConnection};
use cell_core::{Listener, Connection, channel};
use cell_model::protocol::GENOME_REQUEST;
use cell_model::ops::{OpsRequest, OpsResponse, ArchivedOpsRequest};
use anyhow::{Context, Result};
use fd_lock::RwLock;
use rkyv::ser::Serializer;
use rkyv::{Archive, Serialize};
use std::fs::File;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;
use rkyv::ser::serializers::{
    CompositeSerializer, 
    AlignedSerializer, 
    FallbackScratch, 
    SharedSerializeMap, 
    HeapScratch, 
    AllocScratch,
    AllocSerializer 
};
use tracing::{info, warn};
use rkyv::AlignedVec;
use tokio::sync::mpsc::Sender;
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use crate::shm::RingBuffer;
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use cell_model::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::fs::PermissionsExt;
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::io::AsRawFd;
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use crate::transport::ShmConnection;

// Placeholder for Global Metrics - In a real scenario this would be injected or a proper singleton
// For now, we instantiate a static lazy one in the SDK, but we can't access it here easily without circular deps.
// We will assume cell_sdk manages the metrics and passed somehow, or we simply construct a fresh one to satisfy the type system.
// Wait, the requirement was "production-grade".
// Let's rely on cell-sdk metrics if possible. But cell-transport is below cell-sdk.
// We'll define a simple metrics interface or placeholder here since cell-transport shouldn't depend on cell-sdk.
// Actually, cell-transport depends on cell-core/model.
// We will construct empty metrics for now in the response if cell-sdk is not present.
// However, the OpsResponse::Metrics variant expects a cell_sdk::metrics::MetricsSnapshot...
// But OpsResponse is in cell-model.
// We need to move MetricsSnapshot to cell-model or re-export it.
// I will assume MetricsSnapshot is in cell-sdk::metrics, but OpsResponse depends on it.
// This implies cell-model must depend on cell-sdk (Cycle!) or MetricsSnapshot must be in cell-model.
// CORRECT APPROACH: Move MetricsSnapshot definition to cell-model (or define equivalent there).

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const MAX_CONCURRENT_CONNECTIONS: usize = 10_000;

pub struct Membrane;

impl Membrane {
    pub async fn bind_generic<L, F, Req, Resp>(
        mut listener: L,
        handler: F,
        genome_json: Option<String>,
        cell_name: &str,
        consensus_tx: Option<Sender<Vec<u8>>>,
    ) -> Result<()>
    where
        L: Listener + 'static,
        F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>
            + Send + Sync + 'static + Clone,
        Req: Archive + Send,
        Req::Archived:
            for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let g_shared = Arc::new(genome_json);
        let name_owned = cell_name.to_string();
        let c_shared = Arc::new(consensus_tx);
        let start_time = SystemTime::now();

        loop {
            match listener.accept().await {
                Ok(mut connection) => {
                    let permit = match semaphore.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => {
                            warn!("Load Shedding");
                            continue;
                        }
                    };

                    let h = handler.clone();
                    let g = g_shared.clone();
                    let n = name_owned.clone();
                    let c = c_shared.clone();

                    tokio::spawn(async move {
                        let _permit = permit;
                        if let Err(_e) = handle_connection::<F, Req, Resp>(connection, h, g, &n, c, start_time).await {
                             // Suppress errors
                        }
                    });
                }
                Err(e) => {
                    warn!("Listener Accept Error: {:?}", e);
                }
            }
        }
    }

    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
        genome_json: Option<String>,
        consensus_tx: Option<Sender<Vec<u8>>>,
    ) -> Result<()>
    where
        F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>
            + Send + Sync + 'static + Clone,
        Req: Archive + Send,
        Req::Archived:
            for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        let socket_dir = resolve_socket_dir();
        tokio::fs::create_dir_all(&socket_dir).await?;

        let lock_path = socket_dir.join(format!("{}.lock", name));
        let lock_file = File::create(&lock_path).context("Failed to create lock file")?;
        let mut _guard = RwLock::new(lock_file);

        if _guard.try_write().is_err() {
            info!("[{}] Instance already running (Locked).", name);
            return Ok(());
        }

        let socket_path = socket_dir.join(format!("{}.sock", name));
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }

        let listener = UnixListenerAdapter::bind(&socket_path)
            .with_context(|| format!("Failed to bind socket at {:?}", socket_path))?;

        #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
        {
            let perm = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&socket_path, perm);
        }

        info!("[{}] Membrane Active at {:?}", name, socket_path);

        Self::bind_generic::<UnixListenerAdapter, F, Req, Resp>(listener, handler, genome_json, name, consensus_tx).await
    }
}

async fn handle_connection<F, Req, Resp>(
    mut conn: Box<dyn Connection>,
    handler: F,
    genome: Arc<Option<String>>,
    cell_name: &str,
    consensus_tx: Arc<Option<Sender<Vec<u8>>>>,
    start_time: SystemTime,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>> + Send + Sync + 'static,
    Req: Archive + Send,
    Req::Archived:
        for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
    Resp: rkyv::Serialize<AllocSerializer<1024>> + Send,
{
    let mut write_buf = AlignedVec::with_capacity(16 * 1024);

    loop {
        let (channel_id, vesicle) = match conn.recv().await {
            Ok(res) => res,
            Err(_) => return Ok(()),
        };

        let data = vesicle.as_slice();

        #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
        if data == SHM_UPGRADE_REQUEST {
            // Note: Since we are in an async block spawn, downcasting the Box<dyn Connection> 
            // inside the async block is safe regarding Send if Connection is Send.
            // cell_core::Connection requires Send + Sync.
            if let Ok(unix_conn_box) = conn.into_any().downcast::<UnixConnection>() {
                let mut stream = unix_conn_box.into_inner();
                
                let cred = stream.peer_cred()?;
                let my_uid = nix::unistd::getuid().as_raw();
                if cred.uid() != my_uid { return Err(anyhow::anyhow!("UID mismatch")); }

                let challenge: [u8; 32] = rand::random();
                stream.write_all(&challenge).await?;
                
                let mut response = [0u8; 32];
                stream.read_exact(&mut response).await?;
                
                let auth_token = get_shm_auth_token();
                let expected = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
                
                if response != expected.as_bytes()[..32] {
                    return Err(anyhow::anyhow!("Auth failed"));
                }

                let (rx_ring, rx_fd) = RingBuffer::create(&format!("{}_server_rx", cell_name))?;
                let (tx_ring, tx_fd) = RingBuffer::create(&format!("{}_server_tx", cell_name))?;

                stream.write_all(&(SHM_UPGRADE_ACK.len() as u32).to_le_bytes()).await?;
                stream.write_all(SHM_UPGRADE_ACK).await?;

                send_fds(stream.as_raw_fd(), &[rx_fd, tx_fd])?;

                conn = Box::new(ShmConnection::new(rx_ring, tx_ring));
                continue;
            } else {
                return Err(anyhow::anyhow!("SHM request on non-upgradeable transport"));
            }
        }

        if data == GENOME_REQUEST {
            let resp = if let Some(json) = genome.as_ref() { json.as_bytes() } else { &[] };
            conn.send(resp).await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
            continue;
        }

        match channel_id {
            channel::APP => {
                let archived_req = rkyv::check_archived_root::<Req>(data)
                    .map_err(|e| anyhow::anyhow!("Invalid data: {:?}", e))?;

                let response = handler(archived_req).await?;

                let aligned_input = std::mem::take(&mut write_buf);
                
                let mut serializer: CompositeSerializer<
                    AlignedSerializer<AlignedVec>,
                    FallbackScratch<HeapScratch<1024>, AllocScratch>,
                    SharedSerializeMap
                > = CompositeSerializer::new(
                    AlignedSerializer::new(aligned_input),
                    FallbackScratch::default(),
                    SharedSerializeMap::default(),
                );

                serializer.serialize_value(&response)?;
                let aligned_output = serializer.into_serializer().into_inner();
                let bytes = aligned_output.as_slice();
                
                conn.send(bytes).await.map_err(|e| anyhow::anyhow!("{:?}", e))?;

                write_buf = aligned_output;
                write_buf.clear();
            }
            channel::CONSENSUS => {
                if let Some(tx) = consensus_tx.as_ref() {
                    let _ = tx.send(data.to_vec()).await;
                    conn.send(&[]).await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
                } else {
                    conn.send(b"No Consensus").await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
                }
            }
            channel::OPS => {
                let req = rkyv::check_archived_root::<OpsRequest>(data)
                    .map_err(|e| anyhow::anyhow!("Invalid Ops data: {:?}", e))?;
                
                let resp = match req {
                    ArchivedOpsRequest::Ping => OpsResponse::Pong,
                    ArchivedOpsRequest::Status => {
                        let uptime = SystemTime::now().duration_since(start_time).unwrap_or_default().as_secs();
                        OpsResponse::Status {
                            name: cell_name.to_string(),
                            uptime_secs: uptime,
                            memory_usage: 0,
                            consensus_role: if consensus_tx.is_some() { "Enabled".into() } else { "Disabled".into() },
                        }
                    }
                    ArchivedOpsRequest::Metrics => {
                         // Placeholder since we don't have global metrics instance here
                         // In a full impl this connects to cell-sdk metrics
                         OpsResponse::Metrics(cell_model::ops::MetricsSnapshot {
                             requests_total: 0,
                             requests_success: 0,
                             requests_failed: 0,
                             latency_histogram: vec![],
                             connections_active: 0,
                             bytes_sent: 0,
                             bytes_received: 0,
                         })
                    }
                };

                let aligned_input = std::mem::take(&mut write_buf);
                
                let mut serializer: CompositeSerializer<
                    AlignedSerializer<AlignedVec>,
                    FallbackScratch<HeapScratch<1024>, AllocScratch>,
                    SharedSerializeMap
                > = CompositeSerializer::new(
                    AlignedSerializer::new(aligned_input),
                    FallbackScratch::default(),
                    SharedSerializeMap::default(),
                );

                serializer.serialize_value(&resp)?;
                let aligned_output = serializer.into_serializer().into_inner();
                let bytes = aligned_output.as_slice();
                
                conn.send(bytes).await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
                write_buf = aligned_output;
                write_buf.clear();
            }
            _ => {
                conn.send(b"Unknown Channel").await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
            }
        }
    }
}

pub(crate) fn get_shm_auth_token() -> Vec<u8> {
    if let Ok(token) = std::env::var("CELL_SHM_TOKEN") {
        return blake3::hash(token.as_bytes()).as_bytes().to_vec();
    }
    if let Some(home) = dirs::home_dir() {
        let token_path = home.join(".cell/shm.token");
        if let Ok(token) = std::fs::read(&token_path) {
            return blake3::hash(&token).as_bytes().to_vec();
        }
        
        // FIX: Secure token generation. Do NOT fallback to UID hashing.
        let new_token: [u8; 32] = rand::random();
        
        // Attempt to write with restrictive permissions first
        #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
        {
            // Best effort to create directory if missing, though typically handled by runtime
            if let Some(parent) = token_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            
            // Write and chmod
            if std::fs::write(&token_path, &new_token).is_ok() {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                if std::fs::set_permissions(&token_path, perms).is_ok() {
                     return blake3::hash(&new_token).as_bytes().to_vec();
                }
            }
        }
        
        // If we cannot persist a secure token, we return the random one for this session only.
        // This breaks persistence across restarts but maintains security.
        return blake3::hash(&new_token).as_bytes().to_vec();
    }
    
    // Fallback for systems without home dir: strictly random, no persistence.
    // Never hash UID/predictable values.
    let ephemeral: [u8; 32] = rand::random();
    blake3::hash(&ephemeral).as_bytes().to_vec()
}

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
fn send_fds(socket_fd: std::os::unix::io::RawFd, fds: &[std::os::unix::io::RawFd]) -> Result<()> {
    use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
    use std::io::IoSlice;

    let dummy = [0u8; 1];
    let iov = [IoSlice::new(&dummy)];
    let cmsg = ControlMessage::ScmRights(fds);
    sendmsg::<()>(socket_fd, &iov, &[cmsg], MsgFlags::empty(), None)?;
    Ok(())
}