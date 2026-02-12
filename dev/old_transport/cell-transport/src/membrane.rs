// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use crate::transport::{UnixConnection, UnixListenerAdapter};
use anyhow::{Context, Result};
use cell_core::{channel, CellError, Connection, Listener};
use cell_model::macro_coordination::{MacroCoordinationRequest, MacroCoordinationResponse};
use cell_model::ops::{ArchivedOpsRequest, OpsRequest, OpsResponse};
use cell_model::protocol::GENOME_REQUEST;
use fd_lock::RwLock;
use rkyv::ser::serializers::{
    AlignedSerializer, AllocScratch, AllocSerializer, CompositeSerializer, FallbackScratch,
    HeapScratch, SharedSerializeMap,
};
use rkyv::ser::Serializer;
use rkyv::AlignedVec;
use rkyv::Archive;
use std::fs::File;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Sender;
use tokio::sync::Semaphore;
use tracing::{info, warn};

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use crate::shm::RingBuffer;
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use cell_model::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::fs::PermissionsExt;
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::io::AsRawFd;

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
            + Send
            + Sync
            + 'static
            + Clone,
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

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel(1);

        loop {
            tokio::select! {
                res = listener.accept() => {
                    match res {
                        Ok(connection) => {
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
                                     // Suppress connection errors to keep main loop clean
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
            + Send
            + Sync
            + 'static
            + Clone,
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
            .map_err(|_| anyhow::anyhow!("Failed to bind socket"))?;

        #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
        {
            let perm = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&socket_path, perm);
        }

        info!("[{}] Membrane Active at {:?}", name, socket_path);

        Self::bind_generic::<UnixListenerAdapter, F, Req, Resp>(
            listener,
            handler,
            genome_json,
            name,
            consensus_tx,
            coordination_handler,
        )
        .await
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
) -> Result<(), CellError>
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

                let cred = stream.peer_cred().map_err(|_| CellError::AccessDenied)?;
                let my_uid = nix::unistd::getuid().as_raw();
                if cred.uid() != my_uid {
                    return Err(CellError::AccessDenied);
                }

                let challenge: [u8; 32] = rand::random();
                stream
                    .write_all(&challenge)
                    .await
                    .map_err(|_| CellError::IoError)?;

                let mut response = [0u8; 32];
                stream
                    .read_exact(&mut response)
                    .await
                    .map_err(|_| CellError::IoError)?;

                let auth_token =
                    crate::membrane::get_shm_auth_token().map_err(|_| CellError::IoError)?;
                let expected = blake3::hash(&[&challenge, auth_token.as_slice()].concat());

                if response != expected.as_bytes()[..32] {
                    return Err(CellError::AccessDenied);
                }

                let (rx_ring, rx_fd) = RingBuffer::create(&format!("{}_server_rx", cell_name))
                    .map_err(|_| CellError::IoError)?;
                let (tx_ring, tx_fd) = RingBuffer::create(&format!("{}_server_tx", cell_name))
                    .map_err(|_| CellError::IoError)?;

                stream
                    .write_all(&(SHM_UPGRADE_ACK.len() as u32).to_le_bytes())
                    .await
                    .map_err(|_| CellError::IoError)?;
                stream
                    .write_all(SHM_UPGRADE_ACK)
                    .await
                    .map_err(|_| CellError::IoError)?;

                crate::membrane::send_fds(stream.as_raw_fd(), &[rx_fd, tx_fd])
                    .map_err(|_| CellError::IoError)?;

                conn = Box::new(crate::transport::ShmConnection::new(rx_ring, tx_ring));
                continue;
            } else {
                return Err(CellError::CapabilityMissing);
            }
        }

        if data == GENOME_REQUEST {
            let resp = if let Some(json) = genome.as_ref() {
                json.as_bytes()
            } else {
                &[]
            };
            conn.send(resp).await?;
            continue;
        }

        match channel_id {
            channel::APP => {
                let archived_req =
                    rkyv::check_archived_root::<Req>(data).map_err(|_| CellError::InvalidHeader)?;

                let response = match handler(archived_req).await {
                    Ok(r) => r,
                    Err(_) => return Err(CellError::IoError),
                };

                let aligned_input = std::mem::take(&mut write_buf);

                let mut serializer = CompositeSerializer::new(
                    AlignedSerializer::new(aligned_input),
                    FallbackScratch::<HeapScratch<1024>, AllocScratch>::default(),
                    SharedSerializeMap::default(),
                );

                serializer
                    .serialize_value(&response)
                    .map_err(|_| CellError::SerializationFailure)?;
                let aligned_output = serializer.into_serializer().into_inner();
                let bytes = aligned_output.as_slice();

                conn.send(bytes).await?;

                write_buf = aligned_output;
                write_buf.clear();
            }
            channel::CONSENSUS => {
                if let Some(tx) = consensus_tx.as_ref() {
                    let _ = tx.send(data.to_vec()).await;
                    conn.send(&[]).await?;
                } else {
                    conn.send(b"No Consensus").await?;
                }
            }
            channel::OPS => {
                let req = rkyv::check_archived_root::<OpsRequest>(data)
                    .map_err(|_| CellError::InvalidHeader)?;

                let resp = match req {
                    ArchivedOpsRequest::Ping => OpsResponse::Pong,
                    ArchivedOpsRequest::Status => {
                        let uptime = SystemTime::now()
                            .duration_since(start_time)
                            .unwrap_or_default()
                            .as_secs();
                        OpsResponse::Status {
                            name: cell_name.to_string(),
                            uptime_secs: uptime,
                            memory_usage: 0,
                            consensus_role: if consensus_tx.is_some() {
                                "Enabled".into()
                            } else {
                                "Disabled".into()
                            },
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
                        let _ = shutdown.send(());
                        OpsResponse::ShutdownAck
                    }
                    ArchivedOpsRequest::GetSource => {
                        let current_dir = std::env::current_dir().unwrap_or_default();
                        let src_path = current_dir.join("src/main.rs");

                        let bytes = if src_path.exists() {
                            std::fs::read(&src_path).unwrap_or_default()
                        } else {
                            // Fallback for libraries or weird layouts
                            std::fs::read(current_dir.join("src/lib.rs")).unwrap_or_default()
                        };
                        OpsResponse::Source { bytes }
                    }
                };

                let aligned_input = std::mem::take(&mut write_buf);
                let mut serializer = CompositeSerializer::new(
                    AlignedSerializer::new(aligned_input),
                    FallbackScratch::<HeapScratch<1024>, AllocScratch>::default(),
                    SharedSerializeMap::default(),
                );
                serializer
                    .serialize_value(&resp)
                    .map_err(|_| CellError::SerializationFailure)?;
                let aligned_output = serializer.into_serializer().into_inner();
                let bytes = aligned_output.as_slice();

                conn.send(bytes).await?;
                write_buf = aligned_output;
                write_buf.clear();
            }
            channel::MACRO_COORDINATION => {
                if let Some(coord_handler_arc) = coordination_handler.as_ref() {
                    let req = rkyv::check_archived_root::<MacroCoordinationRequest>(data)
                        .map_err(|_| CellError::InvalidHeader)?;

                    let resp = coord_handler_arc
                        .handle(req)
                        .await
                        .map_err(|_| CellError::IoError)?;

                    let resp_bytes = rkyv::to_bytes::<_, 1024>(&resp)
                        .map_err(|_| CellError::SerializationFailure)?
                        .into_vec();
                    conn.send(&resp_bytes).await?;
                } else {
                    let resp = MacroCoordinationResponse::Error {
                        message: "Macro coordination not supported".to_string(),
                    };
                    let resp_bytes = rkyv::to_bytes::<_, 1024>(&resp)
                        .map_err(|_| CellError::SerializationFailure)?
                        .into_vec();
                    conn.send(&resp_bytes).await?;
                }
            }
            _ => {
                conn.send(b"Unknown Channel").await?;
            }
        }
    }
}

// === EXPORTED UTILS FOR AXON SHM BRIDGE ===

pub fn get_shm_auth_token() -> Result<Vec<u8>> {
    if let Ok(token) = std::env::var("CELL_SHM_TOKEN") {
        return Ok(blake3::hash(token.as_bytes()).as_bytes().to_vec());
    }

    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let token_path = home.join(".cell/shm.token");

    if token_path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(&token_path)?;
            let mode = meta.mode() & 0o777;
            let uid = meta.uid();
            let current_uid = nix::unistd::getuid().as_raw();

            if uid != current_uid {
                anyhow::bail!(
                    "SECURITY VIOLATION: SHM token owned by UID {}, expected {}",
                    uid,
                    current_uid
                );
            }
            if mode != 0o600 {
                anyhow::bail!(
                    "SECURITY VIOLATION: SHM token permissions are {:o}, expected 0600",
                    mode
                );
            }
        }

        let token = std::fs::read(&token_path)?;
        return Ok(blake3::hash(&token).as_bytes().to_vec());
    }

    let new_token: [u8; 32] = rand::random();
    let tmp_path = token_path.with_extension("tmp");

    if let Some(parent) = token_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    {
        use std::os::unix::fs::PermissionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;

        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;

        use std::io::Write;
        file.write_all(&new_token)?;
        file.sync_all()?;
    }

    std::fs::rename(&tmp_path, &token_path)?;

    Ok(blake3::hash(&new_token).as_bytes().to_vec())
}

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
pub fn send_fds(
    socket_fd: std::os::unix::io::RawFd,
    fds: &[std::os::unix::io::RawFd],
) -> Result<()> {
    use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
    use std::io::IoSlice;

    let dummy = [0u8; 1];
    let iov = [IoSlice::new(&dummy)];
    let cmsg = ControlMessage::ScmRights(fds);
    sendmsg::<()>(socket_fd, &iov, &[cmsg], MsgFlags::empty(), None)?;
    Ok(())
}
