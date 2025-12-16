// cell-transport/src/membrane.rs
// SPDX-License-Identifier: MIT

use crate::resolve_socket_dir;
use anyhow::{Context, Result};
use cell_core::{channel, CellError, VesicleHeader};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use cell_model::rkyv::Archive;
use rkyv::ser::Serializer;
use std::future::Future;
use std::pin::Pin;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct Membrane;

impl Membrane {
    pub async fn bind<F, Req, Resp>(name: &str, handler: F) -> Result<()>
    where
        F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>
            + Send
            + Sync
            + 'static
            + Clone,
        Req: Archive + Send,
        Req::Archived:
            for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        let socket_dir = resolve_socket_dir();
        let io_dir = socket_dir.join("io");
        std::fs::create_dir_all(&io_dir)?;

        // 1. Create the Input Pipe (The Cell's Ear)
        // In a real implementation, this should be a named pipe (mkfifo).
        // For portability in this snippet, we treat it as a file the router writes to.
        let rx_path = io_dir.join(format!("{}_in", name));

        // Ensure clean state
        if rx_path.exists() {
            // In real FIFO, we wouldn't delete, we'd open.
            // But for file simulation:
            // tokio::fs::remove_file(&rx_path).await?;
        }

        // We create it so the router can find it
        {
            File::create(&rx_path).await?;
        }

        info!("[Membrane] Listening on {:?}", rx_path);

        // 2. Open for reading
        let mut rx = OpenOptions::new()
            .read(true)
            .open(&rx_path)
            .await
            .context("Failed to open input pipe")?;

        // 3. The Event Loop
        loop {
            // A. Read Length Header
            let mut len_buf = [0u8; 4];
            if rx.read_exact(&mut len_buf).await.is_err() {
                // Pipe closed or EOF (Router died?)
                // Sleep and retry logic would go here in a robust impl
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
            let len = u32::from_le_bytes(len_buf) as usize;

            // B. Read Frame
            let mut buf = vec![0u8; len];
            rx.read_exact(&mut buf).await?;

            // C. Parse Header (The Router wrapped it, we unwrap)
            // Format: [Channel: 1] [Source_ID: 8] [Payload: ...]
            // The Membrane needs to know WHO sent it to reply.

            if buf.len() < 9 {
                continue;
            } // Malformed

            let channel = buf[0];
            let source_id = u64::from_le_bytes(buf[1..9].try_into().unwrap());
            let payload = &buf[9..];

            if channel == channel::APP {
                // D. Deserialize
                let archived = unsafe { rkyv::archived_root::<Req>(payload) };

                // E. Handle
                let response = match handler(archived).await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Handler failed: {}", e);
                        continue;
                    }
                };

                // F. Serialize Response
                let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();

                // G. Send Reply
                // We write to a specific output pipe for the Router to pick up.
                // The router watches `io/{name}_out`.
                // We prefix the response with the Target ID (the original Source ID).

                // Response Frame for Router: [TargetID: 8] [Channel: 1] [Payload]

                let tx_path = io_dir.join(format!("{}_out", name));
                let mut tx = OpenOptions::new().append(true).open(&tx_path).await;

                if let Ok(mut pipe) = tx {
                    let total_len = 8 + 1 + resp_bytes.len();
                    pipe.write_all(&(total_len as u32).to_le_bytes()).await?;
                    pipe.write_all(&source_id.to_le_bytes()).await?;
                    pipe.write_u8(channel).await?;
                    pipe.write_all(&resp_bytes).await?;
                }
            }
        }
    }
}
