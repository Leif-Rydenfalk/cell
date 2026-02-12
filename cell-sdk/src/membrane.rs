// SPDX-License-Identifier: MIT
// cell-sdk/src/membrane.rs

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
        Req: Archive + Send + 'static,
        // CRITICAL: Require Archived to be Send + Sync, but NOT the CheckBytes::Error
        for<'a> Req::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>
            + Send
            + Sync
            + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        let std_listener = IoClient::bind_membrane(name)
            .await
            .context("Failed to acquire listener from IO Cell")?;

        std_listener.set_nonblocking(true)?;
        let listener = UnixListener::from_std(std_listener)?;

        info!("[Membrane] {} online (FD inherited)", name);

        let handler = Arc::new(handler);

        loop {
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
        Req: Archive + Send + 'static,
        for<'a> Req::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>
            + Send
            + Sync
            + 'static,
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
            if let Err(e) = stream.read_exact(&mut buf).await {
                error!("Read error: {}", e);
                break;
            }

            if buf.len() < 25 {
                error!("Message too short: {} bytes", buf.len());
                continue;
            }

            let channel = buf[24];
            let payload = &buf[25..];

            if channel == channel::APP {
                let aligned_payload = payload.to_vec();

                // CRITICAL PATTERN: Convert CheckBytes error to String immediately
                // The CheckBytes::Error type is NOT Send, so we must NOT hold it across await points.
                // We use a synchronous block to perform validation, converting any error to String
                // before entering the async error handling path.
                let validation_result: Result<&Req::Archived, String> = {
                    // This block is synchronous - no await points here
                    match rkyv::check_archived_root::<Req>(&aligned_payload) {
                        Ok(archived) => Ok(archived),
                        Err(check_err) => {
                            // IMMEDIATE CONVERSION: Drop check_err by formatting it
                            Err(format!("Request validation failed: {:?}", check_err))
                        }
                    }
                };

                let archived = match validation_result {
                    Ok(a) => a,
                    Err(err_msg) => {
                        error!("{}", err_msg);

                        // Safe to await now - err_msg is a String (Send)
                        let err_bytes = err_msg.into_bytes();
                        let write_fut = async {
                            stream
                                .write_all(&(err_bytes.len() as u32).to_le_bytes())
                                .await?;
                            stream.write_all(&err_bytes).await
                        };

                        if let Err(e) = write_fut.await {
                            error!("Write error: {}", e);
                            return Ok(());
                        }
                        continue;
                    }
                };

                // Now call handler - archived is a simple reference
                let response = match handler(archived).await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Handler Error: {}", e);
                        let err_bytes = format!("Handler error: {}", e).into_bytes();
                        if let Err(e) = stream
                            .write_all(&(err_bytes.len() as u32).to_le_bytes())
                            .await
                        {
                            error!("Write error: {}", e);
                            break;
                        }
                        if let Err(e) = stream.write_all(&err_bytes).await {
                            error!("Write error: {}", e);
                            break;
                        }
                        continue;
                    }
                };

                let resp_bytes = match rkyv::to_bytes::<_, 1024>(&response) {
                    Ok(b) => b.into_vec(),
                    Err(e) => {
                        error!("Response serialization failed: {}", e);
                        continue;
                    }
                };

                let total_len = resp_bytes.len();
                if let Err(e) = stream.write_all(&(total_len as u32).to_le_bytes()).await {
                    error!("Write error: {}", e);
                    break;
                }
                if let Err(e) = stream.write_all(&resp_bytes).await {
                    error!("Write error: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }
}
