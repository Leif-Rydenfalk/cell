// cell-sdk/src/membrane.rs
// SPDX-License-Identifier: MIT

use crate::io_client::IoClient;
use anyhow::{Context, Result};
use cell_core::channel;
use cell_model::rkyv::ser::serializers::AllocSerializer;
use cell_model::rkyv::Archive;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
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
        // 1. Ask IO Cell for the FD
        let std_listener = IoClient::bind_membrane(name)
            .await
            .context("Failed to acquire listener from IO Cell")?;

        std_listener.set_nonblocking(true)?;

        // 2. Convert to Tokio
        let listener = UnixListener::from_std(std_listener)?;

        info!("[Membrane] {} online (FD inherited)", name);

        let handler = Arc::new(handler);

        loop {
            // Removing 'mut' as accept() does not require it on the returned stream variable in this context
            let (stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    error!("Accept error: {}", e);
                    continue;
                }
            };

            let handler = handler.clone();
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
            let mut len_buf = [0u8; 4];
            match stream.read_exact(&mut len_buf).await {
                Ok(_) => (),
                Err(_) => break,
            }
            let len = u32::from_le_bytes(len_buf) as usize;

            let mut buf = vec![0u8; len];
            stream.read_exact(&mut buf).await?;

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
                let total_len = resp_bytes.len();
                stream.write_all(&(total_len as u32).to_le_bytes()).await?;
                stream.write_all(&resp_bytes).await?;
            }
        }
        Ok(())
    }
}
