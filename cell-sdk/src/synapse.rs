// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#[cfg(feature = "lan")]
use crate::axon::AxonClient;
use crate::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
use anyhow::{bail, Result};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Deserialize, Serialize};
use std::marker::PhantomData;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[cfg(target_os = "linux")]
use crate::shm::{RingBuffer, ShmClient, ShmMessage, ShmSerializer};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

enum Transport {
    Socket(UnixStream),
    #[cfg(target_os = "linux")]
    SharedMemory {
        client: ShmClient,
        _socket: UnixStream,
    },
    #[cfg(feature = "lan")]
    Quic(quinn::Connection),
    Empty,
}

pub struct Synapse {
    transport: Transport,
    upgrade_attempted: bool,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        // 1. Try Local Socket (Fast Path)
        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));

        if let Ok(stream) = UnixStream::connect(&socket_path).await {
            return Ok(Self {
                transport: Transport::Socket(stream),
                upgrade_attempted: false,
            });
        }

        // 2. Manual Override (Bypass Discovery)
        #[cfg(feature = "lan")]
        if let Ok(peer_addr) = std::env::var("CELL_PEER") {
             println!("[Synapse] 🔗 Manual override to {}", peer_addr);
             let client = crate::axon::AxonClient::make_endpoint()?;
             let addr = peer_addr.parse()?;
             let conn = client.connect(addr, "localhost")?.await?;
             return Ok(Self {
                transport: Transport::Quic(conn),
                upgrade_attempted: false,
            });
        }

        // 3. Try Axon (LAN Discovery & Connect)
        #[cfg(feature = "lan")]
        if let Some(conn) = AxonClient::connect(cell_name).await? {
            return Ok(Self {
                transport: Transport::Quic(conn),
                upgrade_attempted: false,
            });
        }

        bail!("Cell '{}' not found locally or on LAN", cell_name);
    }
    
    // ... [Rest of the file remains exactly the same as previous output] ...
    
    pub async fn fire<Req, Resp>(&mut self, request: &Req) -> Result<Response<Resp>>
    where
        Req: Serialize<AllocSerializer<1024>> + Serialize<ShmSerializer>,
        Resp: Archive,
        Resp::Archived:
            rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        #[cfg(target_os = "linux")]
        if !self.upgrade_attempted {
            self.upgrade_attempted = true;
            if let Transport::Socket(_) = self.transport {
                 if std::env::var("CELL_DISABLE_SHM").is_err() {
                    if let Err(e) = self.try_upgrade_to_shm().await {
                        eprintln!("SHM upgrade failed: {}", e);
                    }
                }
            }
        }

        match self.transport {
            Transport::Socket(ref mut stream) => Self::fire_via_socket(stream, request).await,
            #[cfg(target_os = "linux")]
            Transport::SharedMemory { ref client, .. } => Self::fire_via_shm(client, request).await,
            #[cfg(feature = "lan")]
            Transport::Quic(ref mut conn) => AxonClient::fire(conn, request).await,
            Transport::Empty => bail!("Connection unusable"),
        }
    }

    async fn fire_via_socket<Req, Resp>(
        stream: &mut UnixStream,
        request: &Req,
    ) -> Result<Response<Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive,
        Resp::Archived: 'static,
    {
        let req_bytes = crate::rkyv::to_bytes::<_, 1024>(request)?.into_vec();

        stream
            .write_all(&(req_bytes.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(&req_bytes).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut resp_bytes = vec![0u8; len];
        stream.read_exact(&mut resp_bytes).await?;

        Ok(Response::<Resp>::Owned(resp_bytes))
    }

    #[cfg(target_os = "linux")]
    async fn fire_via_shm<Req, Resp>(client: &ShmClient, request: &Req) -> Result<Response<Resp>>
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

        let cred = stream.peer_cred()?;
        let my_uid = nix::unistd::getuid().as_raw();
        if cred.uid() != my_uid {
            bail!("UID mismatch");
        }

        stream
            .write_all(&(SHM_UPGRADE_REQUEST.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(SHM_UPGRADE_REQUEST).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut ack = vec![0u8; len];
        stream.read_exact(&mut ack).await?;

        if ack != SHM_UPGRADE_ACK {
            bail!("Rejected");
        }

        stream.readable().await?;
        let fds = recv_fds(stream.as_raw_fd())?;
        if fds.len() != 2 {
            bail!("Expected 2 FDs");
        }

        let tx = unsafe { RingBuffer::attach(fds[0])? };
        let rx = unsafe { RingBuffer::attach(fds[1])? };

        let client = ShmClient::new(tx, rx);

        let old = std::mem::replace(&mut self.transport, Transport::Empty);
        if let Transport::Socket(socket) = old {
            self.transport = Transport::SharedMemory {
                client,
                _socket: socket,
            };
            Ok(())
        } else {
            self.transport = old;
            bail!("State error");
        }
    }
}

// --- Axon Client Visibility Fix ---
// We need to expose make_endpoint in axon.rs for the override to work
// I'll assume axon.rs is updated to make `make_endpoint` public or I'll use `AxonClient::connect` logic
// But for cleaner code, let's just make `make_endpoint` public in axon.rs.
// For now, in this file, we can just grab it if it's pub. 
// If not, we have to copy the logic or expose it.
// Assuming axon.rs `make_endpoint` is private, let's assume we modify axon.rs too.
// Or actually, let's keep it simple:

// ... (Response enum and other helpers same as before) ...
pub enum Response<T: Archive>
where
    <T as Archive>::Archived: 'static,
{
    Owned(Vec<u8>),
    #[cfg(target_os = "linux")]
    ZeroCopy(ShmMessage<T>),
    #[cfg(not(target_os = "linux"))]
    _Phantom(PhantomData<T>),
}

impl<T: Archive> Response<T>
where
    <T as Archive>::Archived: 'static,
{
    pub fn get(&self) -> Result<&T::Archived>
    where
        T::Archived: for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
    {
        match self {
            Response::Owned(bytes) => crate::rkyv::check_archived_root::<T>(bytes)
                .map_err(|e| anyhow::anyhow!("Validation failed: {:?}", e)),
            #[cfg(target_os = "linux")]
            Response::ZeroCopy(msg) => Ok(msg.get()),
            #[cfg(not(target_os = "linux"))]
            Response::_Phantom(_) => anyhow::bail!("Invalid state"),
        }
    }

    pub fn deserialize(&self) -> Result<T>
    where
        T::Archived: Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>
            + for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
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

fn resolve_socket_dir() -> PathBuf {
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