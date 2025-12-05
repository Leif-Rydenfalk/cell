// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use cell_model::protocol::GENOME_REQUEST;
use anyhow::{Context, Result};
use fd_lock::RwLock;
use rkyv::ser::Serializer;
use rkyv::{Archive, Serialize};
use std::fs::File;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, warn};
use tokio::sync::Semaphore;
use socket2::SockRef;
use rkyv::AlignedVec;
use rkyv::ser::serializers::AllocSerializer;

#[cfg(feature = "axon")]
use cell_axon::AxonServer;

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use crate::shm::{RingBuffer, ShmMessage, ShmSerializer};
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
const SOCKET_BUFFER_SIZE: usize = 8 * 1024 * 1024;

pub struct Membrane;

impl Membrane {
    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
        genome_json: Option<String>,
    ) -> Result<()>
    where
        F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>
            + Send + Sync + 'static + Clone,
        Req: Archive + Send,
        Req::Archived:
            for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

        #[cfg(feature = "axon")]
        let axon_server = if std::env::var("CELL_LAN").is_ok() {
            Some(AxonServer::ignite(name).await?)
        } else {
            None
        };

        let h_axon = handler.clone();
        let g_axon = Arc::new(genome_json.clone());
        let s_axon = semaphore.clone();

        let axon_future = async move {
            #[cfg(feature = "axon")]
            if let Some(axon) = axon_server {
                while let Some(conn) = axon.accept().await {
                    let h_inner = h_axon.clone();
                    let g_inner = g_axon.clone();
                    let s_inner = s_axon.clone();

                    tokio::spawn(async move {
                        if let Ok(connection) = conn.await {
                            while let Ok((send, recv)) = connection.accept_bi().await {
                                let permit = match s_inner.clone().try_acquire_owned() {
                                    Ok(p) => p,
                                    Err(_) => continue,
                                };
                                let h_call = h_inner.clone();
                                let g_call = g_inner.clone();
                                tokio::spawn(async move {
                                    let _permit = permit;
                                    let _ = AxonServer::handle_rpc_stream::<F, Req, Resp>(
                                        send, recv, h_call, g_call,
                                    ).await;
                                });
                            }
                        }
                    });
                }
            } else {
                std::future::pending::<()>().await;
            }
            #[cfg(not(feature = "axon"))]
            std::future::pending::<()>().await;
            Ok::<(), anyhow::Error>(())
        };

        let local_future = bind_local::<F, Req, Resp>(name, handler, Arc::new(genome_json), semaphore);

        tokio::select! {
            res = local_future => res,
            _ = axon_future => Ok(()),
        }
    }
}

async fn bind_local<F, Req, Resp>(
    name: &str,
    handler: F,
    genome: Arc<Option<String>>,
    semaphore: Arc<Semaphore>,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>> + Send + Sync + 'static + Clone,
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

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind socket at {:?}", socket_path))?;

    #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
    {
        let perm = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&socket_path, perm)?;
    }

    info!("[{}] Membrane Active at {:?}", name, socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let sock_ref = SockRef::from(&stream);
                let _ = sock_ref.set_recv_buffer_size(SOCKET_BUFFER_SIZE);
                let _ = sock_ref.set_send_buffer_size(SOCKET_BUFFER_SIZE);

                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("[{}] Load Shedding", name);
                        continue;
                    }
                };

                let h = handler.clone();
                let g = genome.clone();
                let cell_name = name.to_string();

                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) =
                        handle_connection::<F, Req, Resp>(stream, h, g, &cell_name).await
                    {
                        let msg = e.to_string();
                        let is_disconnect = msg == "early eof" 
                            || msg.contains("Broken pipe") 
                            || msg.contains("Connection reset");

                        if !is_disconnect {
                            warn!("[{}] Connection Error: {}", cell_name, e);
                        }
                    }
                });
            }
            Err(_) => break,
        }
    }
    Ok(())
}

async fn handle_connection<F, Req, Resp>(
    mut stream: UnixStream,
    handler: F,
    genome: Arc<Option<String>>,
    _cell_name: &str,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>> + Send + Sync + 'static,
    Req: Archive + Send,
    Req::Archived:
        for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
    Resp: rkyv::Serialize<AllocSerializer<1024>> + Send,
{
    let mut read_buf = Vec::with_capacity(16 * 1024);
    let mut write_buf = AlignedVec::with_capacity(16 * 1024);

    loop {
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            return Ok(());
        }

        let len = u32::from_le_bytes(len_buf) as usize;
        
        if read_buf.capacity() < len {
            read_buf.reserve(len - read_buf.capacity());
        }
        read_buf.resize(len, 0); 
        
        if stream.read_exact(&mut read_buf).await.is_err() {
            return Ok(());
        }

        if read_buf == GENOME_REQUEST {
            let resp = if let Some(json) = genome.as_ref() {
                json.as_bytes()
            } else {
                &[]
            };
            stream.write_all(&(resp.len() as u32).to_le_bytes()).await?;
            stream.write_all(resp).await?;
            continue;
        }

        #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
        if read_buf == SHM_UPGRADE_REQUEST {
            if std::env::var("CELL_DISABLE_SHM").is_ok() {
                stream.write_all(&0u32.to_le_bytes()).await?;
                continue;
            }
            return handle_shm_upgrade::<F, Req, Resp>(stream, handler, _cell_name).await;
        }
        #[cfg(not(all(feature = "shm", any(target_os = "linux", target_os = "macos"))))]
        if read_buf == cell_model::protocol::SHM_UPGRADE_REQUEST {
             stream.write_all(&0u32.to_le_bytes()).await?;
             continue;
        }

        handle_socket_rpc::<F, Req, Resp>(&mut stream, &read_buf, &mut write_buf, &handler).await?;
    }
}

async fn handle_socket_rpc<F, Req, Resp>(
    stream: &mut UnixStream,
    request_bytes: &[u8],
    write_buffer: &mut AlignedVec,
    handler: &F,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>,
    Req: Archive,
    Req::Archived: for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
    Resp: rkyv::Serialize<AllocSerializer<1024>>,
{
    let archived_req = rkyv::check_archived_root::<Req>(request_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid data: {:?}", e))?;

    let response = handler(archived_req).await?;

    let aligned_input = std::mem::take(write_buffer);
    
    let mut serializer = rkyv::ser::serializers::CompositeSerializer::new(
        rkyv::ser::serializers::AlignedSerializer::new(aligned_input),
        rkyv::ser::serializers::FallbackScratch::default(),
        rkyv::ser::serializers::SharedSerializeMap::default(),
    );

    serializer.serialize_value(&response)?;
    
    let aligned_output = serializer.into_serializer().into_inner();
    let bytes = aligned_output.as_slice();
    let len_bytes = (bytes.len() as u32).to_le_bytes();

    stream.write_all(&len_bytes).await?;
    stream.write_all(bytes).await?;

    *write_buffer = aligned_output;
    write_buffer.clear();

    Ok(())
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


#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
async fn handle_shm_upgrade<F, Req, Resp>(
    mut stream: UnixStream,
    handler: F,
    cell_name: &str,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>> + Send + Sync + 'static,
    Req: Archive + Send,
    Req::Archived:
        for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
    Resp: Serialize<ShmSerializer> + Send,
{
    let cred = stream.peer_cred()?;
    let my_uid = nix::unistd::getuid().as_raw();

    if cred.uid() != my_uid {
        bail!(
            "Security Alert: SHM request from UID {} mismatch",
            cred.uid()
        );
    }
    
    let challenge: [u8; 32] = rand::random();
    stream.write_all(&challenge).await?;
    
    let mut response = [0u8; 32];
    stream.read_exact(&mut response).await?;
    
    let auth_token = get_shm_auth_token();
    let expected = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
    
    if response != expected.as_bytes()[..32] {
        bail!("Authentication failed for SHM upgrade");
    }

    let (rx_ring, rx_fd) = RingBuffer::create(&format!("{}_server_rx", cell_name))?;
    let (tx_ring, tx_fd) = RingBuffer::create(&format!("{}_server_tx", cell_name))?;

    stream
        .write_all(&(SHM_UPGRADE_ACK.len() as u32).to_le_bytes())
        .await?;
    stream.write_all(SHM_UPGRADE_ACK).await?;

    let raw_fd = stream.as_raw_fd();
    send_fds(raw_fd, &[rx_fd, tx_fd])?;

    serve_zero_copy::<F, Req, Resp>(rx_ring, tx_ring, handler).await
}

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
async fn serve_zero_copy<F, Req, Resp>(
    rx: Arc<RingBuffer>,
    tx: Arc<RingBuffer>,
    handler: F,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>> + Send + Sync + 'static,
    Req: Archive + Send,
    Req::Archived:
        rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    Resp: Serialize<ShmSerializer>,
{
    let mut spin = 0u32;

    loop {
        let request_msg: ShmMessage<Req> = if let Some(msg) = rx.try_read() {
            spin = 0;
            msg
        } else {
            spin += 1;
            if spin < 1000 {
                std::hint::spin_loop();
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(1)).await;
            }
            continue;
        };

        let archived_req = request_msg.get();
        let response = handler(archived_req).await?;
        drop(request_msg);

        let bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();
        let size = bytes.len();

        let mut slot = tx.wait_for_slot(size).await;
        slot.write(&bytes);
        slot.commit(size);
    }
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