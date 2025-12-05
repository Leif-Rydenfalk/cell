// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#[cfg(feature = "axon")]
use crate::axon::AxonClient;
use crate::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
use crate::shm::{ShmSerializer};
use anyhow::{bail, Result};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Deserialize, Serialize};
use rkyv::ser::Serializer;
use rkyv::AlignedVec;
use std::marker::PhantomData;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use std::time::Duration;
use tracing::{info, warn};
use socket2::SockRef;

#[cfg(target_os = "linux")]
use crate::shm::{RingBuffer, ShmClient, ShmMessage};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

// OPTIMIZATION: Large buffers (8MB) to prevent kernel fragmentation on large payloads.
// Standard Unix sockets default to ~128KB, causing context switch storms for MB-sized messages.
const SOCKET_BUFFER_SIZE: usize = 8 * 1024 * 1024;

enum Transport {
    Socket(UnixStream),
    #[cfg(target_os = "linux")]
    SharedMemory {
        client: ShmClient,
        _socket: UnixStream, // Keep socket alive to hold file lock/credentials
    },
    #[cfg(feature = "axon")]
    Quic(quinn::Connection),
    Empty,
}

pub struct Synapse {
    transport: Transport,
    upgrade_attempted: bool,
    // OPTIMIZATION: Persistent buffers to avoid malloc churn.
    // read_buffer: Resizable Vec for incoming data.
    // write_buffer: AlignedVec for zero-copy serialization reuse (rkyv requirement).
    read_buffer: Vec<u8>,
    write_buffer: AlignedVec,
}

impl Synapse {
    /// Automatic discovery with fallback chain: Local → LAN → Manual Override.
    pub async fn grow(cell_name: &str) -> Result<Self> {
        const MAX_RETRIES: u32 = 3;
        const RETRY_DELAY: Duration = Duration::from_secs(1);
        
        info!("[Synapse] Connecting to '{}'...", cell_name);

        for attempt in 1..=MAX_RETRIES {
            // 1. Try Local Socket (Fastest - same machine)
            let socket_dir = resolve_socket_dir();
            let socket_path = socket_dir.join(format!("{}.sock", cell_name));

            if let Ok(stream) = UnixStream::connect(&socket_path).await {
                // Configure kernel buffers
                let sock_ref = SockRef::from(&stream);
                let _ = sock_ref.set_recv_buffer_size(SOCKET_BUFFER_SIZE);
                let _ = sock_ref.set_send_buffer_size(SOCKET_BUFFER_SIZE);

                info!("[Synapse] ✓ Local connection established");
                return Ok(Self {
                    transport: Transport::Socket(stream),
                    upgrade_attempted: false,
                    read_buffer: Vec::with_capacity(64 * 1024),
                    write_buffer: AlignedVec::with_capacity(64 * 1024),
                });
            }

            // 2. Try LAN Discovery (Axon / QUIC)
            #[cfg(feature = "axon")]
            {
                // Manual override check (last resort for cross-network debugging)
                if let Ok(peer_addr) = std::env::var("CELL_PEER") {
                    info!("[Synapse] Using manual override: {}", peer_addr);
                    let client = crate::axon::AxonClient::make_endpoint()?;
                    let addr = peer_addr.parse()?;
                    let conn = client.connect(addr, "localhost")?.await?;
                    return Ok(Self {
                        transport: Transport::Quic(conn),
                        upgrade_attempted: false,
                        read_buffer: Vec::with_capacity(64 * 1024),
                        write_buffer: AlignedVec::with_capacity(64 * 1024),
                    });
                }

                // Automatic Pheromone Discovery
                if let Some(conn) = AxonClient::connect(cell_name).await? {
                    return Ok(Self {
                        transport: Transport::Quic(conn),
                        upgrade_attempted: false,
                        read_buffer: Vec::with_capacity(64 * 1024),
                        write_buffer: AlignedVec::with_capacity(64 * 1024),
                    });
                }
            }
            
            if attempt < MAX_RETRIES {
                tokio::time::sleep(RETRY_DELAY).await;
            }
        }

        bail!("Cell '{}' not found. Ensure it is running.", cell_name);
    }

    /// Sends a request and returns a Zero-Copy response.
    /// 
    /// The returned `Response` borrows the internal read buffer of this `Synapse`,
    /// preventing a heap allocation on the read path.
    pub async fn fire<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>> + Serialize<ShmSerializer>,
        Resp: Archive + 'a, // Added lifetime bound
        Resp::Archived:
            rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        // Opportunistic Upgrade to Shared Memory (Linux Only)
        #[cfg(target_os = "linux")]
        if !self.upgrade_attempted {
            self.upgrade_attempted = true;
            if let Transport::Socket(_) = self.transport {
                // Env var escape hatch for debugging raw sockets
                if std::env::var("CELL_DISABLE_SHM").is_err() {
                    if let Err(_e) = self.try_upgrade_to_shm().await {
                        // Silent failure - we just continue using sockets
                        // warn!("SHM Upgrade Failed: {}", _e);
                    }
                }
            }
        }

        // Security Fix: Enforce timeouts to prevent indefinite hangs on network partition
        const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

        // Split borrows to satisfy borrow checker rules
        let transport = &mut self.transport;
        let r_buf = &mut self.read_buffer;
        let w_buf = &mut self.write_buffer;

        let fut = async {
            match transport {
                Transport::Socket(stream) => Self::fire_via_socket(r_buf, w_buf, stream, request).await,
                #[cfg(target_os = "linux")]
                Transport::SharedMemory { client, .. } => Self::fire_via_shm(client, request).await,
                #[cfg(feature = "axon")]
                Transport::Quic(conn) => AxonClient::fire(conn, request).await,
                Transport::Empty => bail!("Connection unusable"),
            }
        };

        match tokio::time::timeout(RPC_TIMEOUT, fut).await {
            Ok(res) => res,
            Err(_) => bail!("RPC timeout after {:?}", RPC_TIMEOUT),
        }
    }

    async fn fire_via_socket<'a, Req, Resp>(
        read_buffer: &'a mut Vec<u8>,
        write_buffer: &mut AlignedVec,
        stream: &mut UnixStream,
        request: &Req,
    ) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a, // Added lifetime bound
        Resp::Archived: 'static,
    {
        // 1. Serialization (Zero Alloc via Buffer Reuse)
        // We take the aligned buffer out of the struct temporarily to use it.
        let aligned_input = std::mem::take(write_buffer);
        
        // Use CompositeSerializer to serialize directly into existing memory
        let mut serializer = rkyv::ser::serializers::CompositeSerializer::new(
            rkyv::ser::serializers::AlignedSerializer::new(aligned_input),
            rkyv::ser::serializers::FallbackScratch::default(),
            rkyv::ser::serializers::SharedSerializeMap::default(),
        );

        serializer.serialize_value(request)?;
        
        let aligned_output = serializer.into_serializer().into_inner();
        let bytes = aligned_output.as_slice();
        let len_bytes = (bytes.len() as u32).to_le_bytes();

        // 2. Write
        // We use two write calls. write_vectored is technically fewer syscalls, 
        // but write_all guarantees complete delivery for large payloads (preventing deadlocks).
        stream.write_all(&len_bytes).await?;
        stream.write_all(bytes).await?;

        // 3. Recycle Write Buffer
        *write_buffer = aligned_output;
        write_buffer.clear(); 

        // 4. Read (Into reused read_buffer)
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        if read_buffer.capacity() < len {
            read_buffer.reserve(len - read_buffer.capacity());
        }
        // Resizing to len is safe because we immediately read_exact into it.
        // We rely on the OS to fill it or error out.
        read_buffer.resize(len, 0); 
        
        stream.read_exact(read_buffer).await?;

        // OPTIMIZATION: Return Borrowed Slice (Zero Copy Read)
        // This avoids the memcpy back to the user application.
        Ok(Response::Borrowed(read_buffer))
    }

    #[cfg(target_os = "linux")]
    async fn fire_via_shm<'a, Req, Resp>(client: &ShmClient, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<ShmSerializer>,
        Resp: Archive,
        Resp::Archived:
            rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let msg = client.request::<Req, Resp>(request).await?;
        Ok(Response::ZeroCopy(msg))
    }

    #[cfg(target_os = "linux")]
    async fn try_upgrade_to_shm(&mut self) -> Result<()> {
        let stream = match &mut self.transport {
            Transport::Socket(s) => s,
            _ => bail!("Invalid state"),
        };

        // Security Check 1: UID Verification
        // Only allow SHM upgrades if the process owner matches.
        let cred = stream.peer_cred()?;
        let my_uid = nix::unistd::getuid().as_raw();
        if cred.uid() != my_uid {
            bail!("UID mismatch: Peer is {}, I am {}. SHM denied.", cred.uid(), my_uid);
        }

        // 1. Request Upgrade
        stream
            .write_all(&(SHM_UPGRADE_REQUEST.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(SHM_UPGRADE_REQUEST).await?;

        // 2. Receive Challenge (Nonce)
        let mut challenge = [0u8; 32];
        stream.read_exact(&mut challenge).await?;
        
        // Security Check 2: Challenge-Response
        // Prove we have access to the shared secret (file-system protected token).
        let auth_token = crate::membrane::get_shm_auth_token();
        let response = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
        stream.write_all(response.as_bytes()).await?;

        // 4. Await Ack
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut ack = vec![0u8; len];
        stream.read_exact(&mut ack).await?;

        if ack != SHM_UPGRADE_ACK {
            bail!("SHM Upgrade Rejected by Server (Auth Failed)");
        }

        // 5. Receive File Descriptors (Ring Buffers)
        stream.readable().await?;
        let fds = recv_fds(stream.as_raw_fd())?;
        if fds.len() != 2 {
            bail!("Expected 2 FDs, got {}", fds.len());
        }

        // 6. Attach to Shared Memory
        // This maps the memory into our address space.
        let tx = unsafe { RingBuffer::attach(fds[0])? };
        let rx = unsafe { RingBuffer::attach(fds[1])? };

        let client = ShmClient::new(tx, rx);

        let old = std::mem::replace(&mut self.transport, Transport::Empty);
        if let Transport::Socket(socket) = old {
            self.transport = Transport::SharedMemory {
                client,
                _socket: socket, // Keep socket for FD lifecycle management
            };
            Ok(())
        } else {
            self.transport = old;
            bail!("State error during upgrade");
        }
    }
}

// Response now carries a lifetime 'a to allow borrowing from Synapse buffer
pub enum Response<'a, T: Archive>
where
    <T as Archive>::Archived: 'static,
{
    Owned(Vec<u8>), // Network / Legacy
    Borrowed(&'a [u8]), // Zero-Copy Socket Read
    #[cfg(target_os = "linux")]
    ZeroCopy(ShmMessage<T>), // Zero-Copy SHM
    #[cfg(not(target_os = "linux"))]
    _Phantom(PhantomData<&'a T>),
}

impl<'a, T: Archive> Response<'a, T>
where
    <T as Archive>::Archived: 'static,
{
    /// Access the archived data without deserializing.
    /// This is the fastest way to read data.
    pub fn get(&self) -> Result<&T::Archived>
    where
        T::Archived: for<'b> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    {
        match self {
            Response::Owned(bytes) => crate::validate_archived_root::<T>(bytes, "Response::get"),
            Response::Borrowed(bytes) => crate::validate_archived_root::<T>(bytes, "Response::get"),
            #[cfg(target_os = "linux")]
            Response::ZeroCopy(msg) => Ok(msg.get()),
            #[cfg(not(target_os = "linux"))]
            Response::_Phantom(_) => anyhow::bail!("Invalid state"),
        }
    }

    /// Deserializes the data into a standard Rust struct.
    /// This performs a deep copy and allocation.
    pub fn deserialize(&self) -> Result<T>
    where
        T::Archived: Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>
            + for<'b> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    {
        let archived: &T::Archived = self.get()?;
        let mut deserializer = rkyv::de::deserializers::SharedDeserializeMap::new();
        Ok(archived.deserialize(&mut deserializer)?)
    }
}

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