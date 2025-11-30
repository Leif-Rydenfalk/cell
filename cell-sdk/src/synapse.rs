// cell-sdk/src/synapse.rs
// Client-side connection that automatically upgrades to zero-copy shared memory

use crate::protocol::{MitosisRequest, MitosisResponse, SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
use anyhow::{bail, Context, Result};
use rkyv::{Archive, Deserialize, Serialize};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[cfg(target_os = "linux")]
use crate::shm::{RingBuffer, ShmClient, ShmMessage};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

/// Transport layer (socket or shared memory)
enum Transport {
    Socket(UnixStream),
    #[cfg(target_os = "linux")]
    SharedMemory {
        client: ShmClient,
        _socket: UnixStream, // Keep alive for cleanup
    },
}

/// Client connection to a cell
pub struct Synapse {
    transport: Transport,
    upgrade_attempted: bool,
}

impl Synapse {
    /// Connect to a running cell by name
    /// If cell isn't running, asks Mitosis to spawn it
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));

        // 1. Try direct connect (if cell is already running)
        if let Ok(stream) = UnixStream::connect(&socket_path).await {
            return Ok(Self {
                transport: Transport::Socket(stream),
                upgrade_attempted: false,
            });
        }

        // 2. Ask Mitosis to spawn the cell
        let umbilical_path = resolve_umbilical_path();
        let mut umbilical = UnixStream::connect(&umbilical_path)
            .await
            .with_context(|| format!("Failed to connect to Umbilical at {:?}", umbilical_path))?;

        let req = MitosisRequest::Spawn {
            cell_name: cell_name.into(),
        };
        let req_bytes = crate::rkyv::to_bytes::<_, 256>(&req)?.into_vec();

        umbilical
            .write_all(&(req_bytes.len() as u32).to_le_bytes())
            .await?;
        umbilical.write_all(&req_bytes).await?;

        // Read response
        let mut len_buf = [0u8; 4];
        umbilical.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        umbilical.read_exact(&mut buf).await?;

        let resp = crate::rkyv::from_bytes::<MitosisResponse>(&buf)
            .map_err(|e| anyhow::anyhow!("Deserialization failed: {:?}", e))?;

        match resp {
            MitosisResponse::Ok { .. } => {
                // Wait for cell to bind socket
                for _ in 0..50 {
                    if let Ok(stream) = UnixStream::connect(&socket_path).await {
                        return Ok(Self {
                            transport: Transport::Socket(stream),
                            upgrade_attempted: false,
                        });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                bail!(
                    "Cell '{}' spawned but failed to bind within timeout",
                    cell_name
                );
            }
            MitosisResponse::Denied { reason } => bail!("Mitosis Denied: {}", reason),
        }
    }

    /// Send a request and receive a response
    /// Automatically upgrades to zero-copy SHM on first call
    ///
    /// Returns Response<Resp> which may contain zero-copy reference or owned data
    pub async fn fire<Req, Resp>(&mut self, request: &Req) -> Result<Response<Resp>>
    where
        Req: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>>,
        Resp: Archive,
    {
        // Try upgrade on first request
        #[cfg(target_os = "linux")]
        if !self.upgrade_attempted {
            self.upgrade_attempted = true;
            if let Err(e) = self.try_upgrade_to_shm().await {
                eprintln!(
                    "[Synapse] SHM upgrade failed: {} - using socket fallback",
                    e
                );
            } else {
                println!("[Synapse] ✓ Upgraded to zero-copy shared memory");
            }
        }

        // Route to appropriate transport
        match &mut self.transport {
            Transport::Socket(stream) => self.fire_via_socket(stream, request).await,
            #[cfg(target_os = "linux")]
            Transport::SharedMemory { client, .. } => self.fire_via_shm(client, request).await,
        }
    }

    /// Send request via socket (traditional copy-based)
    async fn fire_via_socket<Req, Resp>(
        &self,
        stream: &mut UnixStream,
        request: &Req,
    ) -> Result<Response<Resp>>
    where
        Req: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>>,
        Resp: Archive,
    {
        // 1. Serialize request
        let req_bytes = crate::rkyv::to_bytes::<_, 1024>(request)?.into_vec();

        // 2. Send
        stream
            .write_all(&(req_bytes.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(&req_bytes).await?;

        // 3. Receive
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut resp_bytes = vec![0u8; len];
        stream.read_exact(&mut resp_bytes).await?;

        // 4. Return owned response (will be validated on access)
        Ok(Response::Owned(resp_bytes))
    }

    /// Send request via shared memory (zero-copy)
    #[cfg(target_os = "linux")]
    async fn fire_via_shm<Req, Resp>(
        &self,
        client: &ShmClient,
        request: &Req,
    ) -> Result<Response<Resp>>
    where
        Req: for<'a> Serialize<rkyv::ser::serializers::BufferSerializer<&'a mut [u8]>>,
        Resp: Archive,
    {
        // Send request and get zero-copy response
        let msg = client.request::<Req, Resp>(request).await?;
        Ok(Response::ZeroCopy(msg))
    }

    /// Attempt to upgrade connection to shared memory
    #[cfg(target_os = "linux")]
    async fn try_upgrade_to_shm(&mut self) -> Result<()> {
        let stream = match &mut self.transport {
            Transport::Socket(s) => s,
            _ => bail!("Already upgraded or invalid state"),
        };

        // Security: Verify peer identity
        let cred = stream.peer_cred()?;
        let my_uid = nix::unistd::getuid().as_raw();
        if cred.uid() != my_uid {
            bail!(
                "Security Alert: Connecting to process with UID {} (expected {})",
                cred.uid(),
                my_uid
            );
        }

        // 1. Send upgrade request
        stream
            .write_all(&(SHM_UPGRADE_REQUEST.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(SHM_UPGRADE_REQUEST).await?;

        // 2. Wait for ACK
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut ack = vec![0u8; len];
        stream.read_exact(&mut ack).await?;

        if ack != SHM_UPGRADE_ACK {
            bail!("Server rejected SHM upgrade");
        }

        // 3. Receive file descriptors
        stream.readable().await?;
        let fds = recv_fds(stream.as_raw_fd())?;
        if fds.len() != 2 {
            bail!("Expected 2 FDs (RX/TX), got {}", fds.len());
        }

        // 4. Attach to shared memory rings
        let tx = unsafe { RingBuffer::attach(fds[0])? };
        let rx = unsafe { RingBuffer::attach(fds[1])? };

        let client = ShmClient::new(tx, rx);

        // 5. Replace transport
        let old_transport = std::mem::replace(
            &mut self.transport,
            Transport::Socket(UnixStream::connect("/dev/null").await?), // Dummy
        );

        if let Transport::Socket(socket) = old_transport {
            self.transport = Transport::SharedMemory {
                client,
                _socket: socket,
            };
            Ok(())
        } else {
            bail!("Unexpected transport state during upgrade");
        }
    }
}

/// Response wrapper that can be zero-copy or owned
pub enum Response<T: Archive> {
    /// Socket path: owned bytes (requires validation on access)
    Owned(Vec<u8>),

    /// Shared memory path: zero-copy reference into ring buffer
    #[cfg(target_os = "linux")]
    ZeroCopy(ShmMessage<T>),
}

impl<T: Archive> Response<T> {
    /// Get archived reference (zero-copy when possible)
    pub fn get(&self) -> Result<&T::Archived> {
        match self {
            Response::Owned(bytes) => crate::rkyv::check_archived_root::<T>(bytes)
                .map_err(|e| anyhow::anyhow!("Validation failed: {:?}", e)),
            #[cfg(target_os = "linux")]
            Response::ZeroCopy(msg) => Ok(msg.get()),
        }
    }

    /// Deserialize to owned type (always copies)
    pub fn deserialize(&self) -> Result<T>
    where
        T::Archived: Deserialize<T, crate::rkyv::Infallible>,
    {
        Ok(self.get()?.deserialize(&mut crate::rkyv::Infallible)?)
    }

    /// Check if this is a zero-copy response
    pub fn is_zero_copy(&self) -> bool {
        match self {
            Response::Owned(_) => false,
            #[cfg(target_os = "linux")]
            Response::ZeroCopy(_) => true,
        }
    }
}

// === FD PASSING (Linux-only) ===

#[cfg(target_os = "linux")]
fn recv_fds(socket_fd: std::os::unix::io::RawFd) -> Result<Vec<std::os::unix::io::RawFd>> {
    use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
    use std::io::IoSliceMut;

    let mut dummy = [0u8; 1];
    let mut iov = [IoSliceMut::new(&mut dummy)];
    let mut cmsg_buf = nix::cmsg_space!([std::os::unix::io::RawFd; 4]);

    let msg = recvmsg::<()>(socket_fd, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())?;

    let mut fds = Vec::new();
    for cmsg in msg.cmsgs() {
        if let ControlMessageOwned::ScmRights(received_fds) = cmsg {
            fds.extend(received_fds);
        }
    }
    Ok(fds)
}

// === PATH RESOLUTION ===

fn resolve_socket_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }

    let container_dir = std::path::Path::new("/tmp/cell");
    let container_cord = std::path::Path::new("/tmp/mitosis.sock");

    if container_dir.exists() && container_cord.exists() {
        return container_dir.to_path_buf();
    }

    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run");
    }

    PathBuf::from("/tmp/cell")
}

fn resolve_umbilical_path() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_UMBILICAL") {
        return PathBuf::from(p);
    }

    let container_cord = std::path::Path::new("/tmp/mitosis.sock");
    if container_cord.exists() {
        return container_cord.to_path_buf();
    }

    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run/mitosis.sock");
    }

    PathBuf::from("/tmp/mitosis.sock")
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
    // Connect to cell (will spawn if not running)
    let mut synapse = Synapse::grow("my_cell").await?;

    // Send request (upgrades to SHM automatically)
    let req = MyRequest { id: 42, data: vec![1, 2, 3] };
    let response = synapse.fire::<MyRequest, MyResponse>(&req).await?;

    // Zero-copy access if SHM upgrade succeeded!
    let archived = response.get()?;
    println!("Result: {}", archived.result);

    // Check if zero-copy
    if response.is_zero_copy() {
        println!("Using zero-copy shared memory!");
    }

    Ok(())
}
*/
