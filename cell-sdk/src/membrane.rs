// cell-sdk/src/membrane.rs
// Server-side membrane that binds to Unix socket and handles:
// 1. Normal socket-based RPC (with copy)
// 2. Automatic upgrade to zero-copy shared memory
// 3. High-performance zero-copy request processing

use crate::protocol::{GENOME_REQUEST, SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
use anyhow::{bail, Context, Result};
use fd_lock::RwLock;
use rkyv::ser::Serializer;
use rkyv::{Archive, Deserialize, Serialize};
use std::fs::File;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

// Import zero-copy components
use crate::shm::{RingBuffer, ShmMessage};

pub struct Membrane;

impl Membrane {
    /// Bind the membrane to a socket and start serving requests
    ///
    /// Handler signature: async fn(&ArchivedReq) -> Result<Resp>
    /// - Receives archived (zero-copy) reference to request
    /// - Returns owned response (will be serialized)
    ///
    /// The handler can hold the request reference across await points safely!
    pub async fn bind<F, Fut, Req, Resp>(
        name: &str,
        handler: F,
        genome_json: Option<String>,
    ) -> Result<()>
    where
        F: Fn(&Req::Archived) -> Fut + Send + Sync + 'static + Clone,
        Fut: std::future::Future<Output = Result<Resp>> + Send,
        Req: Archive,
        Resp: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>>
            + Send
            + 'static,
    {
        let socket_dir = resolve_socket_dir();
        tokio::fs::create_dir_all(&socket_dir).await?;

        // 1. Lock file (prevent multiple instances)
        let lock_path = socket_dir.join(format!("{}.lock", name));
        let lock_file = File::create(&lock_path).context("Failed to create lock file")?;
        let mut _guard = RwLock::new(lock_file);

        if _guard.try_write().is_err() {
            println!("[{}] Instance already running. Exiting.", name);
            return Ok(());
        }

        // 2. Bind socket
        let socket_path = socket_dir.join(format!("{}.sock", name));
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind socket at {:?}", socket_path))?;

        // SECURITY: Restrict socket permissions (owner only)
        #[cfg(target_os = "linux")]
        {
            let perm = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&socket_path, perm)?;
        }

        println!("[{}] Membrane Active at {:?}", name, socket_path);

        // 3. Accept connections
        let genome = Arc::new(genome_json);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let h = handler.clone();
                    let g = genome.clone();
                    let cell_name = name.to_string();

                    tokio::spawn(async move {
                        if let Err(e) =
                            handle_connection::<F, Fut, Req, Resp>(stream, h, g, &cell_name).await
                        {
                            // "early eof" is normal client disconnect
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
}

/// Handle a single client connection
/// Supports socket-based RPC and upgrade to shared memory
async fn handle_connection<F, Fut, Req, Resp>(
    mut stream: UnixStream,
    handler: F,
    genome: Arc<Option<String>>,
    cell_name: &str,
) -> Result<()>
where
    F: Fn(&Req::Archived) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Resp>> + Send,
    Req: Archive,
    Resp: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>> + Send,
{
    loop {
        // Read message length
        let mut len_buf = [0u8; 4];
        if stream.read_exact(&mut len_buf).await.is_err() {
            return Ok(()); // Client disconnected
        }

        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        if stream.read_exact(&mut buf).await.is_err() {
            return Ok(());
        }

        // --- PROTOCOL HANDLING ---

        // 1. Genome request (schema introspection)
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

        // 2. Shared memory upgrade request
        if buf == SHM_UPGRADE_REQUEST {
            #[cfg(target_os = "linux")]
            {
                return handle_shm_upgrade::<F, Fut, Req, Resp>(stream, handler, cell_name).await;
            }

            #[cfg(not(target_os = "linux"))]
            {
                // Reject upgrade on non-Linux
                stream.write_all(&0u32.to_le_bytes()).await?;
                continue;
            }
        }

        // 3. Normal socket RPC (with copy)
        handle_socket_rpc::<F, Fut, Req, Resp>(&mut stream, &buf, &handler).await?;
    }
}

/// Handle a single RPC via socket (traditional copy-based path)
async fn handle_socket_rpc<F, Fut, Req, Resp>(
    stream: &mut UnixStream,
    request_bytes: &[u8],
    handler: &F,
) -> Result<()>
where
    F: Fn(&Req::Archived) -> Fut,
    Fut: std::future::Future<Output = Result<Resp>>,
    Req: Archive,
    Resp: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>>,
{
    // 1. Validate and get archived reference
    let archived_req = rkyv::check_archived_root::<Req>(request_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid request: {:?}", e))?;

    // 2. Process request (handler can await freely)
    let response = handler(archived_req).await?;

    // 3. Serialize response
    let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();

    // 4. Send response
    stream
        .write_all(&(resp_bytes.len() as u32).to_le_bytes())
        .await?;
    stream.write_all(&resp_bytes).await?;

    Ok(())
}

/// Handle upgrade to zero-copy shared memory transport
#[cfg(target_os = "linux")]
async fn handle_shm_upgrade<F, Fut, Req, Resp>(
    mut stream: UnixStream,
    handler: F,
    cell_name: &str,
) -> Result<()>
where
    F: Fn(&Req::Archived) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Resp>> + Send,
    Req: Archive,
    Resp: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>> + Send,
{
    // SECURITY: Verify peer identity
    let cred = stream.peer_cred()?;
    let my_uid = nix::unistd::getuid().as_raw();

    if cred.uid() != my_uid {
        bail!(
            "Security Alert: SHM request from UID {} mismatch (expected {})",
            cred.uid(),
            my_uid
        );
    }

    println!("[{}] ⚡ Upgrading to zero-copy shared memory...", cell_name);

    // Create ring buffers (reversed perspective: our RX is client's TX)
    let (rx_ring, rx_fd) = RingBuffer::create(&format!("{}_server_rx", cell_name))?;
    let (tx_ring, tx_fd) = RingBuffer::create(&format!("{}_server_tx", cell_name))?;

    // Send ACK
    stream
        .write_all(&(SHM_UPGRADE_ACK.len() as u32).to_le_bytes())
        .await?;
    stream.write_all(SHM_UPGRADE_ACK).await?;

    // Send file descriptors
    let raw_fd = stream.as_raw_fd();
    send_fds(raw_fd, &[rx_fd, tx_fd])?;

    println!("[{}] ✓ Zero-copy shared memory active", cell_name);

    // Switch to zero-copy serving loop
    serve_zero_copy::<F, Fut, Req, Resp>(rx_ring, tx_ring, handler).await
}

/// Zero-copy serving loop
/// Requests are read in-place, responses serialized directly to shared memory
#[cfg(target_os = "linux")]
async fn serve_zero_copy<F, Fut, Req, Resp>(
    rx: Arc<RingBuffer>,
    tx: Arc<RingBuffer>,
    handler: F,
) -> Result<()>
where
    F: Fn(&Req::Archived) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Resp>> + Send,
    Req: Archive,
    Resp: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>>,
{
    let mut spin = 0u32;

    loop {
        // 1. Try read request (zero-copy)
        let request_msg: ShmMessage<Req> = if let Some(msg) = rx.try_read() {
            spin = 0;
            msg
        } else {
            // Backpressure: wait for data
            spin += 1;
            if spin < 100 {
                std::hint::spin_loop();
            } else if spin < 5000 {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(std::time::Duration::from_micros(50)).await;
            }
            continue;
        };

        // 2. Get archived reference (points into shared memory)
        let archived_req = request_msg.get();

        // 3. Process request
        // CRITICAL: We can hold request_msg across await because it's refcounted!
        let response = match handler(archived_req).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Handler error: {}", e);
                continue; // Drop request, continue serving
            }
        };

        // 4. Drop request early (free slot for next request)
        drop(request_msg);

        // 5. Allocate response slot
        let estimated_size = estimate_size(&response);
        let mut write_slot = tx.wait_for_slot(estimated_size).await;

        // 6. Serialize directly into shared memory
        let mut serializer =
            rkyv::ser::serializers::BufferSerializer::new(write_slot.as_mut_slice());
        match serializer.serialize_value(&response) {
            Ok(_) => {
                let actual_size = serializer.pos();
                write_slot.commit(actual_size);
            }
            Err(e) => {
                eprintln!("Serialization error: {:?}", e);
                // We must still commit to avoid panic in Drop
                write_slot.commit(0);
            }
        }
    }
}

/// Estimate serialized size (conservative)
fn estimate_size<T>(value: &T) -> usize
where
    T: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>>,
{
    let mut dummy = [0u8; 0];
    let mut ser = rkyv::ser::serializers::BufferSerializer::new(&mut dummy[..]);
    let _ = ser.serialize_value(value);
    let size = ser.pos();

    // Add 20% padding for safety
    (size * 120) / 100
}

/// Send file descriptors over Unix domain socket
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

/// Resolve socket directory (container-aware)
pub fn resolve_socket_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }

    let container_socket_dir = std::path::Path::new("/tmp/cell");
    let container_umbilical = std::path::Path::new("/tmp/mitosis.sock");

    // Heuristic: if both container paths exist, assume we're in a container
    if container_socket_dir.exists() && container_umbilical.exists() {
        return container_socket_dir.to_path_buf();
    }

    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run");
    }

    PathBuf::from("/tmp/cell")
}

// === RESPONSE SLOT (for future extensibility) ===

/// Abstract response target
/// Currently socket uses Vec<u8> buffer, SHM uses direct serialization
pub enum ResponseSlot<'a> {
    Socket(Vec<u8>),
    #[cfg(target_os = "linux")]
    Shm(PhantomData<&'a ()>), // Placeholder for direct SHM writes
}

impl<'a> ResponseSlot<'a> {
    pub fn new_socket() -> Self {
        Self::Socket(Vec::new())
    }

    pub fn serialize<T>(&mut self, value: &T) -> Result<()>
    where
        T: for<'s> Serialize<rkyv::ser::serializers::BufferSerializer<&'s mut [u8]>>,
    {
        match self {
            Self::Socket(buf) => {
                let bytes = rkyv::to_bytes::<_, 1024>(value)?.into_vec();
                *buf = bytes;
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Self::Shm(_) => {
                // For SHM, serialization happens directly in serve_zero_copy
                Ok(())
            }
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Socket(buf) => buf,
            #[cfg(target_os = "linux")]
            Self::Shm(_) => Vec::new(),
        }
    }
}

// === USAGE EXAMPLE ===
/*
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
struct MyRequest {
    id: u64,
    data: Vec<u8>,
}

#[derive(Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
struct MyResponse {
    result: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    Membrane::bind::<_, _, MyRequest, MyResponse>(
        "my_cell",
        |req: &ArchivedMyRequest| async move {
            // Zero-copy access to request!
            println!("Processing request {}", req.id);

            Ok(MyResponse {
                result: format!("Processed {}", req.id),
            })
        },
        None,
    ).await
}
*/
