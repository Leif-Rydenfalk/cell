// cell-sdk/src/synapse.rs
// SPDX-License-Identifier: MIT

use crate::response::Response;
use anyhow::{Context, Result};
use cell_core::{channel, VesicleHeader};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use rkyv::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream; // Changed to Net
use tokio::sync::Mutex;

/// A Synapse is now a persistent connection to a Unix Socket.
pub struct Synapse {
    my_id: u64,
    // We wrap the stream in a Mutex because `fire` takes &self (shared),
    // but writing to a stream requires &mut or internal locking.
    // For high-performance concurrent access, we use a Mutex here.
    // In ultra-high perf, we would use a pool of streams.
    stream: Arc<Mutex<UnixStream>>,
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

        // 'tx' is a symlink to the target's 'in' socket
        let tx_path = link_dir.join("tx");

        // Connect to the Socket
        let stream = UnixStream::connect(&tx_path)
            .await
            .context(format!("Failed to connect to synapse {:?}", tx_path))?;

        let my_name = cwd.file_name().unwrap_or_default().to_string_lossy();
        let hash = blake3::hash(my_name.as_bytes());
        let my_id = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());

        Ok(Self {
            my_id,
            stream: Arc::new(Mutex::new(stream)),
        })
    }

    pub async fn fire<'a, Req>(&self, request: &Req) -> Result<Response<'a, Vec<u8>>>
    where
        Req: Serialize<AllocSerializer<1024>>,
    {
        let req_bytes = rkyv::to_bytes::<_, 1024>(request)?.into_vec();
        self.send_raw(channel::APP, &req_bytes).await
    }

    pub async fn fire_on_channel<'a>(
        &self,
        chan: u8,
        payload: &[u8],
    ) -> Result<Response<'a, Vec<u8>>> {
        self.send_raw(chan, payload).await
    }

    async fn send_raw<'a>(&self, chan: u8, payload: &[u8]) -> Result<Response<'a, Vec<u8>>> {
        let mut stream = self.stream.lock().await;

        let header = VesicleHeader {
            target_id: 0,
            source_id: self.my_id,
            ttl: 64,
            flags: 0,
            _pad: [0; 6],
        };

        // Frame: [Len:4] [Header:24] [Channel:1] [Payload:N]
        let total_len = 24 + 1 + payload.len();

        stream.write_all(&(total_len as u32).to_le_bytes()).await?;

        let h_bytes: [u8; 24] = unsafe { std::mem::transmute(header) };
        stream.write_all(&h_bytes).await?;
        stream.write_u8(chan).await?;
        stream.write_all(payload).await?;

        // Wait for Reply
        // Reply Frame: [Len:4] [Payload:N]
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        Ok(Response::Owned(buf))
    }
}
