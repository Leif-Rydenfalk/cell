// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#[cfg(feature = "axon")]
use crate::axon::AxonServer;
use crate::protocol::{GENOME_REQUEST, SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
use crate::shm::{RingBuffer, ShmMessage, ShmSerializer};
use anyhow::{bail, Context, Result};
use fd_lock::RwLock;
use rkyv::ser::Serializer;
use rkyv::{Archive, Serialize};
use std::fs::File;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

// Security Fix #2: Authentication Token
const SHM_AUTH_TOKEN: &[u8] = b"__CELL_SHM_TOKEN__";

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct Membrane;

impl Membrane {
    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
        genome_json: Option<String>,
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
        Resp: rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<1024>>
            + rkyv::Serialize<ShmSerializer>
            + Send
            + 'static,
    {
        // 1. Ignite Axon (LAN) if enabled
        // We do this in parallel with local binding.
        #[cfg(feature = "axon")]
        if std::env::var("CELL_LAN").is_ok() {
            let axon = AxonServer::ignite(name).await?;
            let h = handler.clone();
            let g = Arc::new(genome_json.clone());

            // Spawn the Axon server loop in the background
            tokio::spawn(async move {
                while let Some(conn) = axon.accept().await {
                    let h_inner = h.clone();
                    let g_inner = g.clone();

                    tokio::spawn(async move {
                        if let Ok(connection) = conn.await {
                            while let Ok((send, recv)) = connection.accept_bi().await {
                                let h_call = h_inner.clone();
                                let g_call = g_inner.clone();

                                if let Err(e) = AxonServer::handle_rpc_stream::<F, Req, Resp>(
                                    send, recv, h_call, g_call,
                                )
                                .await
                                {
                                    // eprintln!("Axon RPC Error: {}", e);
                                }
                            }
                        }
                    });
                }
            });
        }

        // 2. Bind Local Socket (Always active for local discovery)
        // This ensures "Same Machine" connections always work via FS/SHM
        bind_local::<F, Req, Resp>(name, handler, Arc::new(genome_json)).await
    }
}

async fn bind_local<F, Req, Resp>(name: &str, handler: F, genome: Arc<Option<String>>) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>> + Send + Sync + 'static + Clone,
    Req: Archive + Send,
    Req::Archived:
        for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
    Resp: rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<1024>>
        + rkyv::Serialize<ShmSerializer>
        + Send
        + 'static,
{
    let socket_dir = resolve_socket_dir();
    tokio::fs::create_dir_all(&socket_dir).await?;

    let lock_path = socket_dir.join(format!("{}.lock", name));
    let lock_file = File::create(&lock_path).context("Failed to create lock file")?;
    let mut _guard = RwLock::new(lock_file);

    if _guard.try_write().is_err() {
        println!("[{}] Instance already running (Locked).", name);
        // We don't exit here if LAN is running, but usually bind_local blocks main thread.
        // If locked, it means another instance is local. We should probably bail.
        return Ok(());
    }

    let socket_path = socket_dir.join(format!("{}.sock", name));
    if socket_path.exists() {
        tokio::fs::remove_file(&socket_path).await?;
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind socket at {:?}", socket_path))?;

    #[cfg(target_os = "linux")]
    {
        let perm = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&socket_path, perm)?;
    }

    println!("[{}] Membrane Active at {:?}", name, socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let h = handler.clone();
                let g = genome.clone();
                let cell_name = name.to_string();

                tokio::spawn(async move {
                    if let Err(e) =
                        handle_connection::<F, Req, Resp>(stream, h, g, &cell_name).await
                    {
                        if e.to_string() != "early eof" {
                            eprintln!("[{}] Connection Error: {}", cell_name, e);
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
    cell_name: &str,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>> + Send + Sync + 'static,
    Req: Archive + Send,
    Req::Archived:
        for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
    Resp: rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<1024>>
        + rkyv::Serialize<ShmSerializer>
        + Send,
{
    loop {
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            return Ok(());
        }

        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        if stream.read_exact(&mut buf).await.is_err() {
            return Ok(());
        }

        if buf == GENOME_REQUEST {
            let resp = if let Some(json) = genome.as_ref() {
                json.as_bytes()
            } else {
                &[]
            };
            stream.write_all(&(resp.len() as u32).to_le_bytes()).await?;
            stream.write_all(resp).await?;
            continue;
        }

        if buf == SHM_UPGRADE_REQUEST {
            #[cfg(target_os = "linux")]
            {
                if std::env::var("CELL_DISABLE_SHM").is_ok() {
                    stream.write_all(&0u32.to_le_bytes()).await?;
                    continue;
                }
                return handle_shm_upgrade::<F, Req, Resp>(stream, handler, cell_name).await;
            }
            #[cfg(not(target_os = "linux"))]
            {
                stream.write_all(&0u32.to_le_bytes()).await?;
                continue;
            }
        }

        handle_socket_rpc::<F, Req, Resp>(&mut stream, &buf, &handler).await?;
    }
}

async fn handle_socket_rpc<F, Req, Resp>(
    stream: &mut UnixStream,
    request_bytes: &[u8],
    handler: &F,
) -> Result<()>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>,
    Req: Archive,
    Req::Archived: for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
    Resp: rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<1024>>,
{
    let archived_req = rkyv::check_archived_root::<Req>(request_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid request: {:?}", e))?;

    let response = handler(archived_req).await?;

    let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();

    stream
        .write_all(&(resp_bytes.len() as u32).to_le_bytes())
        .await?;
    stream.write_all(&resp_bytes).await?;

    Ok(())
}

#[cfg(target_os = "linux")]
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
    // Security Fix #2: UID Verification and Challenge-Response
    let cred = stream.peer_cred()?;
    let my_uid = nix::unistd::getuid().as_raw();

    if cred.uid() != my_uid {
        bail!(
            "Security Alert: SHM request from UID {} mismatch",
            cred.uid()
        );
    }
    
    // 2. Challenge-Response Authentication to prove identity
    let challenge: [u8; 32] = rand::random();
    stream.write_all(&challenge).await?;
    
    let mut response = [0u8; 32];
    stream.read_exact(&mut response).await?;
    
    let expected = blake3::hash(&[&challenge, SHM_AUTH_TOKEN].concat());
    if response != expected.as_bytes()[..32] {
        bail!("Authentication failed for SHM upgrade");
    }

    // println!("[{}]  Upgrading to zero-copy shared memory...", cell_name);

    let (rx_ring, rx_fd) = RingBuffer::create(&format!("{}_server_rx", cell_name))?;
    let (tx_ring, tx_fd) = RingBuffer::create(&format!("{}_server_tx", cell_name))?;

    stream
        .write_all(&(SHM_UPGRADE_ACK.len() as u32).to_le_bytes())
        .await?;
    stream.write_all(SHM_UPGRADE_ACK).await?;

    let raw_fd = stream.as_raw_fd();
    send_fds(raw_fd, &[rx_fd, tx_fd])?;

    // println!("[{}]  Zero-copy shared memory active", cell_name);

    serve_zero_copy::<F, Req, Resp>(rx_ring, tx_ring, handler).await
}

#[cfg(target_os = "linux")]
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

        // Serialize to buffer
        let bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();
        let size = bytes.len();

        let mut slot = tx.wait_for_slot(size).await;
        slot.write(&bytes);
        slot.commit(size);
    }
}

#[cfg(target_os = "linux")]
fn send_fds(socket_fd: std::os::unix::io::RawFd, fds: &[std::os::unix::io::RawFd]) -> Result<()> {
    use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
    use std::io::IoSlice;

    let dummy = [0u8; 1];
    let iov = [IoSlice::new(&dummy)];
    let cmsg = ControlMessage::ScmRights(fds);
    sendmsg::<()>(socket_fd, &iov, &[cmsg], MsgFlags::empty(), None)?;
    Ok(())
}

pub fn resolve_socket_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }
    let container_dir = std::path::Path::new("/tmp/cell");
    if container_dir.exists() {
        return container_dir.to_path_buf();
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run");
    }
    PathBuf::from("/tmp/cell")
}