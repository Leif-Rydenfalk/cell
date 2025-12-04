// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::capsid::Capsid;
use crate::protocol::{MitosisRequest, MitosisResponse};
use crate::ribosome::Ribosome;
use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, warn, error};

pub struct MyceliumRoot {
    socket_dir: PathBuf,
    dna_path: PathBuf,
    umbilical_path: PathBuf,
}

impl MyceliumRoot {
    pub async fn ignite() -> Result<Self> {
        let home = dirs::home_dir().context("Home dir not found")?;

        // Standard Host Paths
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

        // Fix #3: Validation
        let req = crate::rkyv::from_bytes::<MitosisRequest>(&buf)
            .map_err(|e| anyhow::anyhow!("Invalid Protocol: {}", e))?;

        match req {
            MitosisRequest::Spawn { cell_name } => {
                info!("[Root] Request to spawn: {}", cell_name);

                let source = self.dna_path.join(&cell_name);
                if !source.exists() {
                    self.send_resp(
                        &mut stream,
                        MitosisResponse::Denied {
                            reason: format!("DNA not found for {}", cell_name),
                        },
                    )
                    .await?;
                    return Ok(());
                }

                let binary = match Ribosome::synthesize(&source, &cell_name) {
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

                // Capsid maps:
                // binary -> /app/cell
                // socket_dir -> /tmp/cell
                // umbilical_path -> /mitosis.sock
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
                                // This path is used by the CHILD to connect.
                                // The child is in a container where socket_dir is mapped to /tmp/cell.
                                // However, the *caller* of this function might be the Host (Raw) or a Child (Container).
                                //
                                // If the caller is Host (cell-market main), it expects ~/.cell/run/...
                                // If the caller is Child (Exchange), it expects /tmp/cell/...
                                //
                                // ISSUE: We return a single string.
                                // FIX: Synapse logic handles the directory resolution on its end.
                                // We should return just the *name* or a relative path?
                                // Or Synapse ignores the path in Ok and just uses its own resolution?
                                //
                                // Looking at Synapse::grow, it waits for socket_path.
                                // Protocol returns "socket_path".
                                //
                                // Let's return the Host path here. The Container Synapse logic
                                // currently expects to construct the path itself inside `grow`
                                // based on `resolve_socket_dir`.
                                // So the `socket_path` in MitosisResponse::Ok is actually ignored/informational
                                // in the logic I wrote for Synapse::grow (it recalculates it).
                                // Let's send the relative name to be safe/clean.
                                socket_path: cell_name.clone(),
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
        let bytes = crate::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
        stream
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await?;
        stream.write_all(&bytes).await?;
        Ok(())
    }
}