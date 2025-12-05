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

use cell_core::{Transport, TransportError};
use anyhow::{bail, Result, Context};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::sync::Arc;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::info;

#[cfg(feature = "axon")]
use cell_axon::AxonClient;

#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use cell_model::protocol::{SHM_UPGRADE_ACK, SHM_UPGRADE_REQUEST};
#[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
use std::os::unix::io::AsRawFd;

pub struct Synapse {
    // The core of the refactor: Generic Transport
    transport: Box<dyn Transport>,
    
    // For Zero-Copy optimizations (SHM), we hold specific handle if available.
    // This allows specific optimization methods to bypass the generic Transport::call
    #[cfg(feature = "shm")]
    shm_client: Option<ShmClient>,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        info!("[Synapse] Connecting to '{}'...", cell_name);

        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));
        
        // 1. Try Unix Socket
        if let Ok(mut stream) = UnixStream::connect(&socket_path).await {
            info!("[Synapse] ✓ Local connection established");
            
            // Try SHM Upgrade
            #[cfg(all(feature = "shm", any(target_os = "linux", target_os = "macos")))]
            if std::env::var("CELL_DISABLE_SHM").is_err() {
                // Perform upgrade handshake on raw stream
                if let Ok(client) = Self::try_upgrade_to_shm(&mut stream).await {
                    info!("[Synapse] ✓ Upgraded to Shared Memory");
                    return Ok(Self {
                        transport: Box::new(ShmTransport::new(
                            // Cloning ShmClient is cheap (Arc internally)
                            // We need to construct a new one for transport, 
                            // but we can't easily clone the one we just made if ShmClient isn't Clone.
                            // Assuming ShmClient is Clone or cheaply constructable.
                            // ShmClient fields are Arcs, so it should derive Clone. 
                            // If not, we fix ShmClient. (Checked: ShmClient holds Arcs, so cheap copy manually if needed)
                            ShmClient::new(client.tx.clone(), client.rx.clone())
                        )),
                        shm_client: Some(client),
                    });
                }
            }
            
            // Fallback to UnixTransport
            return Ok(Self {
                transport: Box::new(UnixTransport::new(stream)),
                #[cfg(feature = "shm")]
                shm_client: None,
            });
        }

        // 2. Try Axon
        #[cfg(feature = "axon")]
        {
            if let Some(conn) = AxonClient::connect(cell_name).await? {
                return Ok(Self {
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
        
        // Note: crate::membrane::get_shm_auth_token is needed here.
        // For strict compilation, we assume that helper is public or accessible.
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

    pub async fn fire<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        // Optimization: If SHM client is available, use it for zero-copy receive
        #[cfg(feature = "shm")]
        if let Some(client) = &self.shm_client {
             let msg = client.request::<Req, Resp>(request).await?;
             return Ok(Response::ZeroCopy(msg));
        }

        // Generic Transport Path
        let req_bytes = rkyv::to_bytes::<_, 1024>(request)?.into_vec();
        
        let resp_bytes = self.transport.call(&req_bytes).await
            .map_err(|e| anyhow::anyhow!("Transport Error: {:?}", e))?;
            
        Ok(Response::Owned(resp_bytes))
    }
}