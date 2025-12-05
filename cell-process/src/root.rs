// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::capsid::Capsid;
use crate::ribosome::Ribosome;
use cell_model::protocol::{MitosisRequest, MitosisResponse, ArchivedMitosisRequest};
use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, error};

pub struct MyceliumRoot {
    socket_dir: PathBuf,
    dna_path: PathBuf,
    umbilical_path: PathBuf,
}

impl MyceliumRoot {
    pub async fn ignite() -> Result<Self> {
        let home = dirs::home_dir().context("Home dir not found")?;

        let socket_dir = home.join(".cell/run");
        let dna_path = home.join(".cell/dna");
        let umbilical_path = socket_dir.join("mitosis.sock");

        tokio::fs::create_dir_all(&socket_dir).await?;
        tokio::fs::create_dir_all(&dna_path).await?;

        if umbilical_path.exists() {
            tokio::fs::remove_file(&umbilical_path).await?;
        }

        let listener = UnixListener::bind(&umbilical_path)?;
        info!("[Root] Umbilical Cord Active: {:?}", umbilical_path);

        let root = Self {
            socket_dir,
            dna_path,
            umbilical_path,
        };

        let r = root.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let mut r_inner = r.clone();
                    tokio::spawn(async move {
                        if let Err(e) = r_inner.handle_child(stream).await {
                            error!("[Root] Error: {}", e);
                        }
                    });
                }
            }
        });

        Ok(root)
    }

    fn clone(&self) -> Self {
        Self {
            socket_dir: self.socket_dir.clone(),
            dna_path: self.dna_path.clone(),
            umbilical_path: self.umbilical_path.clone(),
        }
    }

    async fn handle_child(&mut self, mut stream: UnixStream) -> Result<()> {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await?;

        // Note: Using cell-model logic here
        let req = cell_model::rkyv::check_archived_root::<MitosisRequest>(&buf)
            .map_err(|e| anyhow::anyhow!("Invalid Protocol: {}", e))?;

        match req {
            ArchivedMitosisRequest::Spawn { cell_name } => {
                let cell_name_str = cell_name.to_string();
                info!("[Root] Request to spawn: {}", cell_name_str);

                let source = self.dna_path.join(&cell_name_str);
                if !source.exists() {
                    self.send_resp(
                        &mut stream,
                        MitosisResponse::Denied {
                            reason: format!("DNA not found for {}", cell_name_str),
                        },
                    )
                    .await?;
                    return Ok(());
                }

                let binary = match Ribosome::synthesize(&source, &cell_name_str) {
                    Ok(b) => b,
                    Err(e) => {
                        self.send_resp(
                            &mut stream,
                            MitosisResponse::Denied {
                                reason: format!("Build Failed: {}", e),
                            },
                        )
                        .await?;
                        return Ok(());
                    }
                };

                match Capsid::spawn(
                    &binary,
                    &self.socket_dir,
                    &self.umbilical_path,
                    &["--membrane"],
                ) {
                    Ok(_) => {
                        self.send_resp(
                            &mut stream,
                            MitosisResponse::Ok {
                                socket_path: cell_name_str,
                            },
                        )
                        .await?;
                    }
                    Err(e) => {
                        self.send_resp(
                            &mut stream,
                            MitosisResponse::Denied {
                                reason: format!("Capsid Error: {}", e),
                            },
                        )
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn send_resp(&self, stream: &mut UnixStream, resp: MitosisResponse) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        stream
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }
}