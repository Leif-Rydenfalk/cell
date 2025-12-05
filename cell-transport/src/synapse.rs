// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::response::Response;
use crate::resolve_socket_dir;
use cell_model::Vesicle;
use anyhow::{bail, Result};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use rkyv::ser::Serializer;
use rkyv::AlignedVec;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use std::time::Duration;
use tracing::info;
use socket2::SockRef;

#[cfg(feature = "axon")]
use cell_axon::AxonClient;

#[cfg(all(feature = "shm", target_os = "linux"))]
use crate::shm::{ShmClient, RingBuffer, ShmSerializer};
#[cfg(all(feature = "shm", target_os = "linux"))]
use cell_model::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
#[cfg(all(feature = "shm", target_os = "linux"))]
use std::os::unix::io::AsRawFd;

const SOCKET_BUFFER_SIZE: usize = 8 * 1024 * 1024;

enum Transport {
    Socket(UnixStream),
    
    #[cfg(all(feature = "shm", target_os = "linux"))]
    SharedMemory {
        client: ShmClient,
        _socket: UnixStream,
    },
    
    #[cfg(feature = "axon")]
    Quic(quinn::Connection),
    
    Empty,
}

pub struct Synapse {
    transport: Transport,
    upgrade_attempted: bool,
    read_buffer: Vec<u8>,
    write_buffer: AlignedVec,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        const MAX_RETRIES: u32 = 3;
        const RETRY_DELAY: Duration = Duration::from_secs(1);
        
        info!("[Synapse] Connecting to '{}'...", cell_name);

        for attempt in 1..=MAX_RETRIES {
            // 1. Try Socket
            let socket_dir = resolve_socket_dir();
            let socket_path = socket_dir.join(format!("{}.sock", cell_name));

            if let Ok(stream) = UnixStream::connect(&socket_path).await {
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

            // 2. Try Axon (Network)
            #[cfg(feature = "axon")]
            {
                if let Ok(peer_addr) = std::env::var("CELL_PEER") {
                    info!("[Synapse] Using manual override: {}", peer_addr);
                    let client = cell_axon::AxonClient::make_endpoint()?;
                    let addr = peer_addr.parse()?;
                    let conn = client.connect(addr, "localhost")?.await?;
                    return Ok(Self {
                        transport: Transport::Quic(conn),
                        upgrade_attempted: false,
                        read_buffer: Vec::with_capacity(64 * 1024),
                        write_buffer: AlignedVec::with_capacity(64 * 1024),
                    });
                }

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

    pub async fn fire<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        // Conditional bounds hack for SHM
        // Req: Serialize<ShmSerializer> is only needed if shm enabled
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        // Try SHM Upgrade
        #[cfg(all(feature = "shm", target_os = "linux"))]
        if !self.upgrade_attempted {
            self.upgrade_attempted = true;
            if let Transport::Socket(_) = self.transport {
                if std::env::var("CELL_DISABLE_SHM").is_err() {
                    if let Err(_e) = self.try_upgrade_to_shm().await {
                        // Upgrade failed, fall back to socket silently
                    }
                }
            }
        }

        const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

        let transport = &mut self.transport;
        let r_buf = &mut self.read_buffer;
        let w_buf = &mut self.write_buffer;

        let fut = async {
            match transport {
                Transport::Socket(stream) => Self::fire_via_socket(r_buf, w_buf, stream, request).await,
                
                #[cfg(all(feature = "shm", target_os = "linux"))]
                Transport::SharedMemory { client, .. } => {
                    // We must serialize using ShmSerializer here. 
                    // To keep the trait bounds simple in the signature above, we perform a trick:
                    // Since ShmSerializer is type aliased to AllocSerializer<1024>, they are actually the SAME type.
                    // So Req already implements it.
                    let msg = client.request::<Req, Resp>(request).await?;
                    Ok(Response::ZeroCopy(msg))
                },
                
                #[cfg(feature = "axon")]
                Transport::Quic(conn) => {
                    let bytes = AxonClient::fire(conn, request).await?;
                    Ok(Response::Owned(bytes))
                },
                
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
        Resp: Archive + 'a,
        Resp::Archived: 'static,
    {
        let aligned_input = std::mem::take(write_buffer);
        
        let mut serializer = rkyv::ser::serializers::CompositeSerializer::new(
            rkyv::ser::serializers::AlignedSerializer::new(aligned_input),
            rkyv::ser::serializers::FallbackScratch::default(),
            rkyv::ser::serializers::SharedSerializeMap::default(),
        );

        serializer.serialize_value(request)?;
        
        let aligned_output = serializer.into_serializer().into_inner();
        let bytes = aligned_output.as_slice();
        let len_bytes = (bytes.len() as u32).to_le_bytes();

        stream.write_all(&len_bytes).await?;
        stream.write_all(bytes).await?;

        *write_buffer = aligned_output;
        write_buffer.clear(); 

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        if read_buffer.capacity() < len {
            read_buffer.reserve(len - read_buffer.capacity());
        }
        read_buffer.resize(len, 0); 
        
        stream.read_exact(read_buffer).await?;

        Ok(Response::Borrowed(read_buffer))
    }

    #[cfg(all(feature = "shm", target_os = "linux"))]
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

        stream.write_all(&(SHM_UPGRADE_REQUEST.len() as u32).to_le_bytes()).await?;
        stream.write_all(SHM_UPGRADE_REQUEST).await?;

        let mut challenge = [0u8; 32];
        stream.read_exact(&mut challenge).await?;
        
        // We need to access get_shm_auth_token from membrane module
        let auth_token = crate::membrane::get_shm_auth_token();
        let response = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
        stream.write_all(response.as_bytes()).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut ack = vec![0u8; len];
        stream.read_exact(&mut ack).await?;

        if ack != SHM_UPGRADE_ACK {
            bail!("SHM Upgrade Rejected");
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

#[cfg(all(feature = "shm", target_os = "linux"))]
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