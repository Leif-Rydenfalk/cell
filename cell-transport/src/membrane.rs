// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use crate::transport::UnixListenerAdapter;
use cell_core::{Listener, Connection, channel};
use cell_model::protocol::GENOME_REQUEST;
use cell_model::ops::{OpsRequest, OpsResponse};
use anyhow::{Context, Result};
use fd_lock::RwLock;
use rkyv::ser::Serializer;
use rkyv::{Archive, Serialize};
use std::fs::File;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;
use rkyv::ser::serializers::AllocSerializer;
use tracing::{info, warn};
use rkyv::AlignedVec;
use tokio::sync::mpsc::Sender;
use std::time::SystemTime;

#[cfg(feature = "axon")]
use cell_axon::AxonServer;

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use crate::shm::{RingBuffer, ShmSerializer};
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use cell_model::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::fs::PermissionsExt;
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::io::AsRawFd;
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use anyhow::bail;

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

        Self::bind_generic(listener, handler, genome_json, name, consensus_tx).await
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

        if data == GENOME_REQUEST {
            let resp = if let Some(json) = genome.as_ref() { json.as_bytes() } else { &[] };
            conn.send(resp).await?;
            continue;
        }

        match channel_id {
            channel::APP => {
                let archived_req = rkyv::check_archived_root::<Req>(data)
                    .map_err(|e| anyhow::anyhow!("Invalid data: {:?}", e))?;

                let response = handler(archived_req).await?;

                let aligned_input = std::mem::take(&mut write_buf);
                let mut serializer = rkyv::ser::serializers::CompositeSerializer::new(
                    rkyv::ser::serializers::AlignedSerializer::new(aligned_input),
                    rkyv::ser::serializers::FallbackScratch::default(),
                    rkyv::ser::serializers::SharedSerializeMap::default(),
                );
                serializer.serialize_value(&response)?;
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
                    .map_err(|e| anyhow::anyhow!("Invalid Ops data: {:?}", e))?;
                
                let resp = match req {
                    cell_model::rkyv::Archived::<OpsRequest>::Ping => OpsResponse::Pong,
                    cell_model::rkyv::Archived::<OpsRequest>::Status => {
                        let uptime = SystemTime::now().duration_since(start_time).unwrap_or_default().as_secs();
                        OpsResponse::Status {
                            name: cell_name.to_string(),
                            uptime_secs: uptime,
                            memory_usage: 0, // Placeholder
                            consensus_role: if consensus_tx.is_some() { "Enabled".into() } else { "Disabled".into() },
                        }
                    }
                };

                let aligned_input = std::mem::take(&mut write_buf);
                let mut serializer = rkyv::ser::serializers::CompositeSerializer::new(
                    rkyv::ser::serializers::AlignedSerializer::new(aligned_input),
                    rkyv::ser::serializers::FallbackScratch::default(),
                    rkyv::ser::serializers::SharedSerializeMap::default(),
                );
                serializer.serialize_value(&resp)?;
                let aligned_output = serializer.into_serializer().into_inner();
                let bytes = aligned_output.as_slice();
                
                conn.send(bytes).await?;
                write_buf = aligned_output;
                write_buf.clear();
            }
            _ => {
                conn.send(b"Unknown Channel").await?;
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
        let new_token: [u8; 32] = rand::random();
        if std::fs::write(&token_path, &new_token).is_ok() {
            #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o600);
                let _ = std::fs::set_permissions(&token_path, perms);
            }
            return blake3::hash(&new_token).as_bytes().to_vec();
        }
    }
    let uid = users::get_current_uid();
    blake3::hash(&uid.to_le_bytes()).as_bytes().to_vec()
}