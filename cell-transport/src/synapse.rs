// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use crate::response::Response;
use crate::transport::UnixTransport;
#[cfg(feature = "axon")]
use crate::transport::QuicTransport;
#[cfg(feature = "shm")]
use crate::transport::ShmTransport;
#[cfg(feature = "shm")]
use crate::shm::{RingBuffer, ShmClient};

use cell_core::{Transport, TransportError, channel};
use anyhow::{bail, Result, Context};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use std::time::Duration;

#[cfg(feature = "axon")]
use cell_axon::AxonClient;

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use cell_model::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::io::AsRawFd;

pub struct Synapse {
    // Stored for healing/regrowing
    cell_name: String,
    
    transport: Box<dyn Transport>,
    
    #[cfg(feature = "shm")]
    shm_client: Option<ShmClient>,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        info!("[Synapse] Connecting to '{}'...", cell_name);

        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));
        
        if let Ok(mut stream) = UnixStream::connect(&socket_path).await {
            info!("[Synapse] ✓ Local connection established");
            
            #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
            if std::env::var("CELL_DISABLE_SHM").is_err() {
                if let Ok(client) = Self::try_upgrade_to_shm(&mut stream).await {
                    info!("[Synapse] ✓ Upgraded to Shared Memory");
                    return Ok(Self {
                        cell_name: cell_name.to_string(),
                        transport: Box::new(ShmTransport::new(
                            ShmClient::new(client.tx.clone(), client.rx.clone())
                        )),
                        shm_client: Some(client),
                    });
                }
            }
            
            return Ok(Self {
                cell_name: cell_name.to_string(),
                transport: Box::new(UnixTransport::new(stream)),
                #[cfg(feature = "shm")]
                shm_client: None,
            });
        }

        #[cfg(feature = "axon")]
        {
            if let Some(conn) = AxonClient::connect(cell_name).await? {
                return Ok(Self {
                    cell_name: cell_name.to_string(),
                    transport: Box::new(QuicTransport::new(conn)),
                    #[cfg(feature = "shm")]
                    shm_client: None,
                });
            }
        }
        
        bail!("Cell '{}' not found", cell_name);
    }
    
    #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
    async fn try_upgrade_to_shm(stream: &mut UnixStream) -> Result<ShmClient> {
        let cred = stream.peer_cred()?;
        let my_uid = nix::unistd::getuid().as_raw();
        if cred.uid() != my_uid { bail!("UID mismatch"); }

        stream.write_all(&(SHM_UPGRADE_REQUEST.len() as u32).to_le_bytes()).await?;
        stream.write_all(SHM_UPGRADE_REQUEST).await?;

        let mut challenge = [0u8; 32];
        stream.read_exact(&mut challenge).await?;
        
        let auth_token = crate::membrane::get_shm_auth_token();
        let response = blake3::hash(&[&challenge, auth_token.as_slice()].concat());
        stream.write_all(response.as_bytes()).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut ack = vec![0u8; len];
        stream.read_exact(&mut ack).await?;

        if ack != SHM_UPGRADE_ACK { bail!("SHM Upgrade Rejected"); }

        stream.readable().await?;
        let fds = Self::recv_fds(stream.as_raw_fd())?;
        if fds.len() != 2 { bail!("Expected 2 FDs"); }

        let tx = unsafe { RingBuffer::attach(fds[0])? };
        let rx = unsafe { RingBuffer::attach(fds[1])? };

        Ok(ShmClient::new(tx, rx))
    }

    #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
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

    // HEALING LOGIC
    async fn heal(&mut self) -> Result<()> {
        warn!("[Synapse] Connection lost to '{}'. Healing...", self.cell_name);
        
        const RECONNECT_ATTEMPTS: usize = 5;
        const BASE_DELAY: u64 = 200;

        for i in 0..RECONNECT_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(BASE_DELAY * (1 << i))).await;
            
            match Synapse::grow(&self.cell_name).await {
                Ok(new_syn) => {
                    info!("[Synapse] Reconnected to '{}'.", self.cell_name);
                    self.transport = new_syn.transport;
                    #[cfg(feature = "shm")]
                    { self.shm_client = new_syn.shm_client; }
                    return Ok(());
                },
                Err(e) => {
                    warn!("[Synapse] Reconnect attempt {}/{} failed: {}", i+1, RECONNECT_ATTEMPTS, e);
                }
            }
        }
        
        bail!("Failed to heal connection to '{}' after {} attempts", self.cell_name, RECONNECT_ATTEMPTS);
    }

    pub async fn fire_on_channel(&mut self, channel_id: u8, data: &[u8]) -> Result<Response<Vec<u8>>> {
        let mut retries = 0;
        loop {
            // Raw Shm Path (Note: healing might change this option)
            #[cfg(feature = "shm")]
            if let Some(client) = &self.shm_client {
                 match client.request_raw(data, channel_id).await {
                     Ok(msg) => return Ok(Response::Owned(msg.get_bytes().to_vec())),
                     Err(_) => {
                         // SHM Failure (e.g. ring corruption or peer death)
                         // Fallthrough to heal
                     }
                 }
            }

            // Generic Transport Path
            // We construct the frame here. Note: If using SHM transport but no client optimization,
            // we use call(). 
            let mut frame = Vec::with_capacity(1 + data.len());
            frame.push(channel_id);
            frame.extend_from_slice(data);
            
            match self.transport.call(&frame).await {
                Ok(resp_bytes) => return Ok(Response::Owned(resp_bytes)),
                Err(e) => {
                    match e {
                        TransportError::Io | TransportError::ConnectionClosed | TransportError::Timeout => {
                            if retries > 0 { return Err(anyhow::anyhow!("Transport Error: {:?}", e)); }
                            if let Err(heal_err) = self.heal().await {
                                return Err(anyhow::anyhow!("Transport Error: {:?}, Healing Failed: {}", e, heal_err));
                            }
                            retries += 1;
                            continue;
                        },
                        _ => return Err(anyhow::anyhow!("Transport Error: {:?}", e)),
                    }
                }
            }
        }
    }

    pub async fn fire<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let channel = channel::APP;
        let mut retries = 0;

        loop {
            #[cfg(feature = "shm")]
            if let Some(client) = &self.shm_client {
                 match client.request::<Req, Resp>(request, channel).await {
                     Ok(msg) => return Ok(Response::ZeroCopy(msg)),
                     Err(_) => {
                         // Fallthrough to heal
                     }
                 }
            }

            let req_bytes = match rkyv::to_bytes::<_, 1024>(request) {
                Ok(b) => b.into_vec(),
                Err(e) => return Err(anyhow::anyhow!("Serialization error: {}", e)),
            };
            
            let mut frame = Vec::with_capacity(1 + req_bytes.len());
            frame.push(channel);
            frame.extend_from_slice(&req_bytes);
            
            match self.transport.call(&frame).await {
                Ok(resp_bytes) => return Ok(Response::Owned(resp_bytes)),
                Err(e) => {
                    match e {
                        TransportError::Io | TransportError::ConnectionClosed | TransportError::Timeout => {
                            if retries > 0 { return Err(anyhow::anyhow!("Transport Error: {:?}", e)); }
                            if let Err(heal_err) = self.heal().await {
                                return Err(anyhow::anyhow!("Transport Error: {:?}, Healing Failed: {}", e, heal_err));
                            }
                            retries += 1;
                            continue;
                        },
                        _ => return Err(anyhow::anyhow!("Transport Error: {:?}", e)),
                    }
                }
            }
        }
    }
}