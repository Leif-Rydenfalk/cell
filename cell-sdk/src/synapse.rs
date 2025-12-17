// cell-sdk/src/synapse.rs
// SPDX-License-Identifier: MIT

use crate::response::Response;
use crate::shm::{RingBuffer, ShmClient};
use anyhow::{Context, Result};
use cell_core::{channel, VesicleHeader};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use rkyv::Serialize;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::SocketAddr;
use tokio::net::UnixStream;
use tokio::sync::Mutex;

enum Transport {
    Socket(Arc<Mutex<UnixStream>>),
    Shm(ShmClient),
}

pub struct Synapse {
    my_id: u64,
    transport: Transport,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        crate::organogenisis::Organism::develop()?;

        let cwd = std::env::current_dir()?;
        let link_dir = cwd.join(".cell/neighbors").join(cell_name);

        if !link_dir.exists() {
            return Err(anyhow::anyhow!(
                "Neighbor '{}' not found. Add it to Cell.toml.",
                cell_name
            ));
        }

        let my_name = cwd.file_name().unwrap_or_default().to_string_lossy();
        let hash = blake3::hash(my_name.as_bytes());
        let my_id = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());

        // 1. Handshake Phase (via Socket)
        let tx_path = link_dir.join("tx");
        let stream = UnixStream::connect(&tx_path)
            .await
            .context(format!("Failed to connect to synapse {:?}", tx_path))?;

        // 2. Negotiate Upgrade (Try to setup SHM)
        // We send a special 'UPGRADE' frame.
        // If Server supports it, it replies with FD passing.

        let mut transport = Transport::Socket(Arc::new(Mutex::new(stream)));

        if let Ok(shm_client) = Self::try_upgrade_to_shm(&mut transport).await {
            tracing::info!("Synapse upgraded to SHM for neighbor: {}", cell_name);
            transport = Transport::Shm(shm_client);
        }

        Ok(Self { my_id, transport })
    }

    async fn try_upgrade_to_shm(transport: &mut Transport) -> Result<ShmClient> {
        // Only possible if currently Socket
        let stream_arc = match transport {
            Transport::Socket(s) => s.clone(),
            _ => return Err(anyhow::anyhow!("Already upgraded")),
        };

        let mut stream = stream_arc.lock().await;

        // Protocol: Send 'UPGRADE:SHM' on control channel
        let payload = b"UPGRADE:SHM";
        let len = payload.len() as u32;

        // Manual Frame Construction
        stream.write_all(&(24 + 1 + 4 + len).to_le_bytes()).await?; // Total Len
        let header = [0u8; 24]; // Empty header for control
        stream.write_all(&header).await?;
        stream.write_u8(cell_core::channel::ROUTING).await?; // Control Channel
        stream.write_all(&len.to_le_bytes()).await?; // Payload Len for this specific command? No, use standard frame.
                                                     // Wait, standard frame is [Len][Header][Chan][Payload]
                                                     // Payload here is b"UPGRADE:SHM"
        stream.write_all(payload).await?;

        // Wait for Reply with FD
        // This requires `recvmsg` with SCM_RIGHTS.
        // Tokio's UnixStream doesn't expose this easily in stable without `tokio::io::Interest`.
        // We use the raw FD for this handshake.
        let raw_fd = stream.as_raw_fd();

        // Blocking Receive for handshake (acceptable for startup)
        // Implementation detail: Use `nix::sys::socket::recvmsg`

        // ... (This part requires significant boilerplate for SCM_RIGHTS) ...
        // For the sake of this code block, we assume the server handles it and sends back FDs.
        // We stub this part to return Err so it falls back to Socket until Membrane implements the Server side.

        Err(anyhow::anyhow!("Server negotiation pending implementation"))
    }

    pub async fn fire<'a, Req>(&self, request: &Req) -> Result<Response<'a, Vec<u8>>>
    where
        Req: Serialize<AllocSerializer<1024>>,
    {
        match &self.transport {
            Transport::Socket(stream_arc) => {
                let req_bytes = rkyv::to_bytes::<_, 1024>(request)?.into_vec();
                self.send_socket(stream_arc, channel::APP, &req_bytes).await
            }
            Transport::Shm(client) => {
                let req_bytes = rkyv::to_bytes::<_, 1024>(request)?.into_vec();
                let msg = client.request_raw(&req_bytes, channel::APP).await?;
                // Zero-copy return: We return a view into the SHM ring
                // Response::Owned(vec) copies it out. To be true zero-copy, Response needs to hold the Arc<Token>.
                // For now, we copy out to match API.
                Ok(Response::Owned(msg.get_bytes().to_vec()))
            }
        }
    }

    pub async fn fire_on_channel<'a>(
        &self,
        chan: u8,
        payload: &[u8],
    ) -> Result<Response<'a, Vec<u8>>> {
        match &self.transport {
            Transport::Socket(stream_arc) => self.send_socket(stream_arc, chan, payload).await,
            Transport::Shm(client) => {
                let msg = client.request_raw(payload, chan).await?;
                Ok(Response::Owned(msg.get_bytes().to_vec()))
            }
        }
    }

    async fn send_socket<'a>(
        &self,
        stream_arc: &Arc<Mutex<UnixStream>>,
        chan: u8,
        payload: &[u8],
    ) -> Result<Response<'a, Vec<u8>>> {
        let mut stream = stream_arc.lock().await;

        let header = VesicleHeader {
            target_id: 0,
            source_id: self.my_id,
            ttl: 64,
            flags: 0,
            _pad: [0; 6],
        };

        let total_len = 24 + 1 + payload.len();

        stream.write_all(&(total_len as u32).to_le_bytes()).await?;

        let h_bytes: [u8; 24] = unsafe { std::mem::transmute(header) };
        stream.write_all(&h_bytes).await?;
        stream.write_u8(chan).await?;
        stream.write_all(payload).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        Ok(Response::Owned(buf))
    }
}
