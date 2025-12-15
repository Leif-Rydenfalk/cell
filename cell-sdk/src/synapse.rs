// cell-sdk/src/synapse.rs
// SPDX-License-Identifier: MIT
// The Neural Connection. Pure File I/O. No Networking.

use crate::resolve_socket_dir;
use crate::response::Response;
use anyhow::{anyhow, bail, Context, Result};
use cell_core::channel;
use cell_core::VesicleHeader;
use cell_model::rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A Synapse is a bidirectional file connection.
/// It knows NOTHING about sockets, TCP, Lasers, or Satellites.
/// It only knows how to read/write to the paths defined in Cell.toml.
pub struct Synapse {
    rx: File,
    tx: File,
    routing_target: Option<u64>,
    request_id: AtomicU32,
}

impl Synapse {
    /// Connect to a neighbor.
    /// This resolves strictly via the filesystem topology created by the CLI.
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = resolve_socket_dir();
        let neighbors_dir = socket_dir.join("neighbors");

        // STRATEGY 1: Direct Neighbor (Fastest)
        // Check if .cell/run/<inst>/neighbors/<name> exists
        let direct_path = neighbors_dir.join(cell_name);
        if direct_path.exists() {
            match connect_pipes(&direct_path).await {
                Ok((rx, tx)) => return Ok(Self::new_direct(rx, tx)),
                Err(_) => {
                    // It exists but connection failed.
                    // Check autostart policy (The "Socket Finder" Logic)
                    Self::try_autostart(cell_name).await?;
                    // Retry once
                    let (rx, tx) = connect_pipes(&direct_path).await?;
                    return Ok(Self::new_direct(rx, tx));
                }
            }
        }

        // STRATEGY 2: Routed via Default Gateway (The Network)
        // If not local, ask 'default' (The Router Cell)
        let gateway_path = neighbors_dir.join("default");
        if gateway_path.exists() {
            let (rx, tx) = connect_pipes(&gateway_path)
                .await
                .context("Default gateway defined but unreachable")?;

            return Ok(Self::new_routed(rx, tx, cell_name));
        }

        // STRATEGY 3: Last Resort Autogenesis
        // If no router and no socket, check if we can spawn it from source in cwd
        if Self::try_autostart(cell_name).await.is_ok() {
            // It might have appeared now
            if direct_path.exists() {
                let (rx, tx) = connect_pipes(&direct_path).await?;
                return Ok(Self::new_direct(rx, tx));
            }
        }

        bail!(
            "Cell '{}' unreachable. No direct link, no gateway, and autostart failed.",
            cell_name
        );
    }

    fn new_direct(rx: File, tx: File) -> Self {
        Self {
            rx,
            tx,
            routing_target: None,
            request_id: AtomicU32::new(0),
        }
    }

    fn new_routed(rx: File, tx: File, target_name: &str) -> Self {
        let hash = blake3::hash(target_name.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash.as_bytes()[..8]);
        let target_id = u64::from_le_bytes(bytes);

        Self {
            rx,
            tx,
            routing_target: Some(target_id),
            request_id: AtomicU32::new(0),
        }
    }

    async fn try_autostart(cell_name: &str) -> Result<()> {
        let manifest_path = PathBuf::from("Cell.toml");
        let path_to_check = if manifest_path.exists() {
            manifest_path
        } else {
            PathBuf::from("Cargo.toml")
        };

        if !path_to_check.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&path_to_check)?;
        let manifest: cell_model::manifest::CellManifest = toml::from_str(&content)?;

        if let Some(config) = manifest.neighbors.get(cell_name) {
            let (path, autostart) = match config {
                cell_model::manifest::NeighborConfig::Path(p) => (p, false),
                cell_model::manifest::NeighborConfig::Detailed { path, autostart } => {
                    (path, *autostart)
                }
            };

            if autostart {
                println!(
                    "[Synapse] Autostarting neighbor '{}' from '{}'...",
                    cell_name, path
                );
                let instance = std::env::var("CELL_INSTANCE").unwrap_or_default();

                let mut cmd = std::process::Command::new("cell");
                cmd.arg("run");
                cmd.arg("--path").arg(path);
                cmd.arg("--instance").arg(instance);
                cmd.arg("--release");

                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                cmd.spawn().context("Failed to spawn cell CLI")?;

                tokio::time::sleep(Duration::from_millis(500)).await;
                return Ok(());
            }
        }
        Ok(())
    }

    pub async fn fire<'a, Req, Resp>(
        &'a mut self,
        request: &Req,
    ) -> Result<Response<'a, Resp>, cell_core::CellError>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a,
        Resp::Archived:
            rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let req_bytes = match rkyv::to_bytes::<_, 1024>(request) {
            Ok(b) => b.into_vec(),
            Err(_) => return Err(cell_core::CellError::SerializationFailure),
        };

        if let Some(target) = self.routing_target {
            // ROUTED REQUEST (Wrap in Header)
            let header = VesicleHeader {
                target_id: target,
                source_id: 0,
                ttl: 64,
                _pad: [0; 7],
            };

            // Manual serialization of header to avoid rkyv overhead
            let mut head_bytes = Vec::with_capacity(24);
            head_bytes.extend_from_slice(&header.target_id.to_le_bytes());
            head_bytes.extend_from_slice(&header.source_id.to_le_bytes());
            head_bytes.push(header.ttl);
            head_bytes.extend_from_slice(&header._pad);

            // Write to local file (pipe)
            // Router Cell reads this, sees header, and handles the Laser Logic.
            self.send_frame(channel::ROUTING, &head_bytes, &req_bytes)
                .await?;
        } else {
            // DIRECT REQUEST
            self.send_frame(channel::APP, &[], &req_bytes).await?;
        }

        // Wait for response from file
        let resp_bytes = self.recv_frame().await?;
        Ok(Response::Owned(resp_bytes))
    }

    async fn send_frame(
        &mut self,
        channel: u8,
        header: &[u8],
        payload: &[u8],
    ) -> Result<(), cell_core::CellError> {
        let total_len = 1 + header.len() + payload.len();
        let len_bytes = (total_len as u32).to_le_bytes();

        self.tx
            .write_all(&len_bytes)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        self.tx
            .write_u8(channel)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        if !header.is_empty() {
            self.tx
                .write_all(header)
                .await
                .map_err(|_| cell_core::CellError::IoError)?;
        }
        self.tx
            .write_all(payload)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        self.tx
            .flush()
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        Ok(())
    }

    async fn recv_frame(&mut self) -> Result<Vec<u8>, cell_core::CellError> {
        let mut len_buf = [0u8; 4];
        self.rx
            .read_exact(&mut len_buf)
            .await
            .map_err(|_| cell_core::CellError::ConnectionReset)?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        self.rx
            .read_exact(&mut buf)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;

        if len > 0 {
            Ok(buf[1..].to_vec()) // Strip channel byte
        } else {
            Ok(vec![])
        }
    }
}

async fn connect_pipes(dir: &std::path::Path) -> Result<(File, File)> {
    let tx_path = dir.join("tx");
    let rx_path = dir.join("rx");

    // Open TX first (Write)
    let tx = OpenOptions::new()
        .write(true)
        .open(&tx_path)
        .await
        .context(format!("Failed to open TX pipe at {:?}", tx_path))?;

    // Open RX (Read)
    let rx = OpenOptions::new()
        .read(true)
        .open(&rx_path)
        .await
        .context(format!("Failed to open RX pipe at {:?}", rx_path))?;

    Ok((rx, tx))
}
