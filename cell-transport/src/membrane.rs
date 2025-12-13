// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::transport::UnixListenerAdapter;
use cell_core::{Listener, Connection, channel, CellError};
use cell_model::ops::{OpsRequest, OpsResponse, ArchivedOpsRequest};
use anyhow::Result;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::Archive;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;
use std::time::SystemTime;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const MAX_CONCURRENT_CONNECTIONS: usize = 10_000;

pub struct Membrane;

impl Membrane {
    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
    ) -> Result<()>
    where
        F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>
            + Send + Sync + 'static + Clone,
        Req: Archive + Send,
        Req::Archived: for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        let socket_dir = crate::resolve_socket_dir();
        tokio::fs::create_dir_all(&socket_dir).await?;

        let socket_path = socket_dir.join(format!("{}.sock", name));
        if socket_path.exists() {
            tokio::fs::remove_file(&socket_path).await?;
        }

        let mut listener = UnixListenerAdapter::bind(&socket_path)?;
        
        tracing::info!("[{}] Membrane bound to {:?}", name, socket_path);

        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let start_time = SystemTime::now();

        loop {
            match listener.accept().await {
                Ok(connection) => {
                    let permit = match semaphore.clone().try_acquire_owned() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };

                    let h = handler.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ = handle_connection::<F, Req, Resp>(connection, h, start_time).await;
                    });
                }
                Err(_) => continue,
            }
        }
    }
}

async fn handle_connection<F, Req, Resp>(
    mut conn: Box<dyn Connection>,
    handler: F,
    start_time: SystemTime,
) -> Result<(), CellError>
where
    F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>,
    Req: Archive,
    Req::Archived: for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
    Resp: rkyv::Serialize<AllocSerializer<1024>>,
{
    loop {
        let (channel_id, vesicle) = conn.recv().await?;
        let data = vesicle.as_slice();

        match channel_id {
            channel::APP => {
                let archived_req = rkyv::check_archived_root::<Req>(data)
                    .map_err(|_| CellError::InvalidHeader)?;

                let response = handler(archived_req).await
                    .map_err(|_| CellError::IoError)?;

                let bytes = rkyv::to_bytes::<_, 1024>(&response)
                    .map_err(|_| CellError::SerializationFailure)?
                    .into_vec();
                
                conn.send(&bytes).await?;
            }
            channel::OPS => {
                let req = rkyv::check_archived_root::<OpsRequest>(data)
                    .map_err(|_| CellError::InvalidHeader)?;
                
                let resp = match req {
                    ArchivedOpsRequest::Ping => OpsResponse::Pong,
                    ArchivedOpsRequest::Status => {
                        let uptime = SystemTime::now()
                            .duration_since(start_time)
                            .unwrap_or_default()
                            .as_secs();
                        OpsResponse::Status {
                            name: "cell".to_string(),
                            uptime_secs: uptime,
                        }
                    }
                    ArchivedOpsRequest::Shutdown => {
                        conn.send(&rkyv::to_bytes::<_, 1024>(&OpsResponse::ShutdownAck)?.into_vec()).await?;
                        std::process::exit(0);
                    }
                };

                let bytes = rkyv::to_bytes::<_, 1024>(&resp)?.into_vec();
                conn.send(&bytes).await?;
            }
            _ => {
                conn.send(b"Unknown Channel").await?;
            }
        }
    }
}