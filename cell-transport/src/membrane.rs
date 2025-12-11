// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use crate::transport::{UnixListenerAdapter, UnixConnection};
use cell_core::{Listener, Connection, channel};
use cell_model::protocol::GENOME_REQUEST;
use cell_model::ops::{OpsRequest, OpsResponse, ArchivedOpsRequest};
use cell_model::macro_coordination::{MacroCoordinationRequest, MacroCoordinationResponse, ArchivedMacroCoordinationRequest};
use anyhow::{Context, Result, bail};
use fd_lock::RwLock;
use rkyv::ser::Serializer;
use rkyv::Archive;
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

use crate::coordination::CoordinationHandler;

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
        coordination_handler: Option<Arc<CoordinationHandler>>,
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
        let coord_shared = Arc::new(coordination_handler);
        let start_time = SystemTime::now();
        
        // Handle shutdown signal
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel(1);

        loop {
            tokio::select! {
                res = listener.accept() => {
                    match res {
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
                            let ch = coord_shared.clone();
                            let shutdown = shutdown_tx.clone();

                            tokio::spawn(async move {
                                let _permit = permit;
                                if let Err(_e) = handle_connection::<F, Req, Resp>(connection, h, g, &n, c, start_time, ch, shutdown).await {
                                     // Suppress errors
                                }
                            });
                        }
                        Err(e) => {
                            warn!("Listener Accept Error: {:?}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("[Membrane] Shutdown signal received. Exiting accept loop.");
                    break;
                }
            }
        }
        Ok(())
    }

    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
        genome_json: Option<String>,
        consensus_tx: Option<Sender<Vec<u8>>>,
        coordination_handler: Option<Arc<CoordinationHandler>>,
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

        Self::bind_generic::<UnixListenerAdapter, F, Req, Resp>(listener, handler, genome_json, name, consensus_tx, coordination_handler).await
    }
}

async fn handle_connection<F, Req, Resp>(
    mut conn: Box<dyn Connection>,
    handler: F,
    genome: Arc<Option<String>>,
    cell_name: &str,
    consensus_tx: Arc<Option<Sender<Vec<u8>>>>,
    start_time: SystemTime,
    coordination_handler: Arc<Option<Arc<CoordinationHandler>>>,
    shutdown: tokio::sync::broadcast::Sender<()>,
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
            if let Ok(unix_conn_box) = conn.into_any().downcast::<UnixConnection>() {
                let mut stream = unix_conn_box.into_inner();
                
                let cred = stream.peer_cred()?;
                let my_uid = nix::unistd::getuid().as_raw();
                if cred.uid() != my_uid { return Err(anyhow::anyhow!("UID mismatch")); }

                let challenge: [u8; 32] = rand::random();
                stream.write_all(&challenge).await?;
                
                let mut response = [0u8; 32];
                stream.read_exact(&mut response).await?;
                
                let auth_token = crate::membrane::get_shm_auth_token().context("CRITICAL: Failed to retrieve secure SHM token")?;
                let expected = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
                
                if response != expected.as_bytes()[..32] {
                    return Err(anyhow::anyhow!("Auth failed"));
                }

                let (rx_ring, rx_fd) = RingBuffer::create(&format!("{}_server_rx", cell_name))?;
                let (tx_ring, tx_fd) = RingBuffer::create(&format!("{}_server_tx", cell_name))?;

                stream.write_all(&(SHM_UPGRADE_ACK.len() as u32).to_le_bytes()).await?;
                stream.write_all(SHM_UPGRADE_ACK).await?;

                crate::membrane::send_fds(stream.as_raw_fd(), &[rx_fd, tx_fd])?;

                conn = Box::new(crate::transport::ShmConnection::new(rx_ring, tx_ring));
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
                    ArchivedOpsRequest::Shutdown => {
                        // Broadcast shutdown signal
                        let _ = shutdown.send(());
                        OpsResponse::ShutdownAck
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
            channel::MACRO_COORDINATION => {
                if let Some(coord_handler_arc) = coordination_handler.as_ref() {
                    let req = rkyv::check_archived_root::<MacroCoordinationRequest>(data)
                        .map_err(|e| anyhow::anyhow!("Invalid macro coordination request: {:?}", e))?;
                    
                    let resp = coord_handler_arc.handle(req).await?;
                    
                    let resp_bytes = rkyv::to_bytes::<_, 1024>(&resp)?.into_vec();
                    conn.send(&resp_bytes).await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
                } else {
                    let resp = MacroCoordinationResponse::Error { message: "Macro coordination not supported".to_string() };
                    let resp_bytes = rkyv::to_bytes::<_, 1024>(&resp)?.into_vec();
                    conn.send(&resp_bytes).await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
                }
            }
            _ => {
                conn.send(b"Unknown Channel").await.map_err(|e| anyhow::anyhow!("{:?}", e))?;
            }
        }
    }
}

pub(crate) fn get_shm_auth_token() -> Result<Vec<u8>> {
    if let Ok(token) = std::env::var("CELL_SHM_TOKEN") {
        return Ok(blake3::hash(token.as_bytes()).as_bytes().to_vec());
    }

    let home = dirs::home_dir().context("Cannot determine home directory for SHM token storage")?;
    let token_path = home.join(".cell/shm.token");

    if token_path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(&token_path).context("Failed to stat SHM token")?;
            let mode = meta.mode() & 0o777;
            let uid = meta.uid();
            let current_uid = nix::unistd::getuid().as_raw();

            if uid != current_uid {
                bail!("SECURITY VIOLATION: SHM token owned by UID {}, expected {}", uid, current_uid);
            }
            if mode != 0o600 {
                bail!("SECURITY VIOLATION: SHM token permissions are {:o}, expected 0600 (rw-------)", mode);
            }
        }

        let token = std::fs::read(&token_path).context("Failed to read SHM token")?;
        return Ok(blake3::hash(&token).as_bytes().to_vec());
    }

    let new_token: [u8; 32] = rand::random();
    let tmp_path = token_path.with_extension("tmp");
    
    if let Some(parent) = token_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create .cell directory")?;
    }

    {
        use std::os::unix::fs::PermissionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .context("Failed to create temporary SHM token file")?;
        
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("Failed to set 0600 permissions on SHM token")?;
            
        use std::io::Write;
        file.write_all(&new_token).context("Failed to write SHM token data")?;
        file.sync_all().context("Failed to sync SHM token to disk")?;
    }

    std::fs::rename(&tmp_path, &token_path).context("Failed to atomically install SHM token")?;

    Ok(blake3::hash(&new_token).as_bytes().to_vec())
}

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
pub fn send_fds(socket_fd: std::os::unix::io::RawFd, fds: &[std::os::unix::io::RawFd]) -> Result<()> {
    use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
    use std::io::IoSlice;

    let dummy = [0u8; 1];
    let iov = [IoSlice::new(&dummy)];
    let cmsg = ControlMessage::ScmRights(fds);
    sendmsg::<()>(socket_fd, &iov, &[cmsg], MsgFlags::empty(), None)?;
    Ok(())
}