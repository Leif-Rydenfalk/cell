// cell-sdk/src/membrane.rs
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use cell_core::channel;
use cell_model::rkyv::ser::serializers::AllocSerializer;
use cell_model::rkyv::Archive;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream}; // Changed to Net
use tracing::{error, info};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct Membrane;

impl Membrane {
    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
        _opts: Option<()>,
        _conf: Option<()>,
        _coord: Option<()>,
    ) -> Result<()>
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
        let cwd = std::env::current_dir()?;
        let io_dir = cwd.join(".cell/io");
        std::fs::create_dir_all(&io_dir)?;

        let rx_path = io_dir.join("in");

        // Clean up old socket file if it exists
        if rx_path.exists() {
            std::fs::remove_file(&rx_path)?;
        }

        // Bind Unix Listener
        let listener = UnixListener::bind(&rx_path)
            .context(format!("Failed to bind Membrane to {:?}", rx_path))?;

        info!("[Membrane] {} listening on {:?}", name, rx_path);

        let handler = Arc::new(handler);

        loop {
            // Accept connection (High performance, async)
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    error!("Accept error: {}", e);
                    continue;
                }
            };

            let handler = handler.clone();

            // Spawn task per connection to handle concurrency
            tokio::spawn(async move {
                let _ = Self::handle_connection::<F, Req, Resp>(stream, handler).await;
            });
        }
    }

    async fn handle_connection<F, Req, Resp>(mut stream: UnixStream, handler: Arc<F>) -> Result<()>
    where
        F: Fn(&Req::Archived) -> BoxFuture<Result<Resp>> + Send + Sync + 'static,
        Req: Archive + Send,
        Req::Archived:
            for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        loop {
            // 1. Read Length Header
            let mut len_buf = [0u8; 4];
            match stream.read_exact(&mut len_buf).await {
                Ok(_) => (),
                Err(_) => break, // EOF or Error, close connection
            }
            let len = u32::from_le_bytes(len_buf) as usize;

            // 2. Read Frame
            let mut buf = vec![0u8; len];
            stream.read_exact(&mut buf).await?;

            // Frame: [Header:24] [Channel:1] [Payload]
            if buf.len() < 25 {
                break;
            }

            let channel = buf[24];
            let payload = &buf[25..];

            if channel == channel::APP {
                let archived = unsafe { rkyv::archived_root::<Req>(payload) };

                let response = match handler(archived).await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Handler Error: {}", e);
                        break;
                    }
                };

                let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)?.into_vec();

                // Reply on the SAME stream (Request/Response correlation is implicit in TCP/UDS)
                let total_len = resp_bytes.len();
                stream.write_all(&(total_len as u32).to_le_bytes()).await?;
                stream.write_all(&resp_bytes).await?;
            }
        }
        Ok(())
    }
}
