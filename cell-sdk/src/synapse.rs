// cell-sdk/src/synapse.rs
// The Neural Connection. Dumb, fast, file-based.

use crate::resolve_socket_dir;
use crate::response::Response;
use anyhow::{anyhow, bail, Context, Result};
use cell_core::channel;
use cell_core::VesicleHeader;
use cell_model::rkyv::ser::serializers::AllocSerializer;
use rkyv::{Archive, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub struct Synapse {
    stream: UnixStream,
    routing_target: Option<u64>, // If set, we wrap traffic in routing headers
    request_id: AtomicU32,
}

impl Synapse {
    /// Connect to a neighbor.
    /// This resolves strictly via the filesystem topology created by the CLI.
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = resolve_socket_dir();
        let neighbors_dir = socket_dir.join("neighbors");

        // STRATEGY 1: Direct Neighbor (Fastest)
        // Check if ./neighbors/<name> exists and is a socket
        let direct_path = neighbors_dir.join(cell_name);
        if direct_path.exists() {
            match connect_with_retry(&direct_path, false).await {
                Ok(stream) => return Ok(Self::new_direct(stream)),
                Err(_) => {
                    // It exists but connection failed.
                    // Check autostart policy.
                    Self::try_autostart(cell_name).await?;
                    // Retry once
                    let stream = connect_with_retry(&direct_path, true).await?;
                    return Ok(Self::new_direct(stream));
                }
            }
        }

        // STRATEGY 2: Routed via Default Gateway (Fractal)
        // If not local, ask 'default' (the Router)
        let gateway_path = neighbors_dir.join("default");
        if gateway_path.exists() {
            // We connect to the gateway, but we set the routing target
            let stream = connect_with_retry(&gateway_path, false)
                .await
                .context("Default gateway defined but unreachable")?;

            return Ok(Self::new_routed(stream, cell_name));
        }

        // STRATEGY 3: Last Resort Autogenesis
        // If no router and no socket, check if we can spawn it from source in cwd
        if Self::try_autostart(cell_name).await.is_ok() {
            // It might have appeared now
            if direct_path.exists() {
                let stream = connect_with_retry(&direct_path, true).await?;
                return Ok(Self::new_direct(stream));
            }
        }

        bail!(
            "Cell '{}' unreachable. No direct link, no gateway, and autostart failed.",
            cell_name
        );
    }

    fn new_direct(stream: UnixStream) -> Self {
        Self {
            stream,
            routing_target: None,
            request_id: AtomicU32::new(0),
        }
    }

    fn new_routed(stream: UnixStream, target_name: &str) -> Self {
        let hash = blake3::hash(target_name.as_bytes());
        // Take first 8 bytes as u64 ID
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash.as_bytes()[..8]);
        let target_id = u64::from_le_bytes(bytes);

        Self {
            stream,
            routing_target: Some(target_id),
            request_id: AtomicU32::new(0),
        }
    }

    async fn try_autostart(cell_name: &str) -> Result<()> {
        // Read Cell.toml/Cargo.toml from CWD to find path
        // This is a "dumb" read - we assume we are in the cell root
        let manifest_path = PathBuf::from("Cell.toml");
        let path_to_check = if manifest_path.exists() {
            manifest_path
        } else {
            PathBuf::from("Cargo.toml")
        };

        if !path_to_check.exists() {
            return Ok(());
        } // Can't check config

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

                // We shell out to `cell run`
                // We must preserve the instance ID to link it correctly!
                let instance = std::env::var("CELL_INSTANCE").unwrap_or_default();

                let mut cmd = std::process::Command::new("cell");
                cmd.arg("run");
                cmd.arg("--path").arg(path);
                cmd.arg("--instance").arg(instance);
                cmd.arg("--release"); // Best effort optimization

                // Detach
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());

                cmd.spawn().context("Failed to spawn cell CLI")?;

                // Give it a moment to boot
                tokio::time::sleep(Duration::from_millis(500)).await;
                return Ok(());
            } else {
                println!(
                    "[Synapse] Neighbor '{}' found but not running and autostart=false.",
                    cell_name
                );
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
            // ROUTED REQUEST
            // Frame: [0x04 (Route)][Header][Payload]
            let header = VesicleHeader {
                target_id: target,
                source_id: 0, // Anonymity/Router fills this? Or we put our own hash?
                ttl: 64,
                _pad: [0; 7],
            };

            // Serialize header
            // Manual struct serialization to avoid rkyv overhead for protocol frame
            // Layout: target(8) + source(8) + ttl(1) + pad(7) = 24 bytes
            let mut head_bytes = Vec::with_capacity(24);
            head_bytes.extend_from_slice(&header.target_id.to_le_bytes());
            head_bytes.extend_from_slice(&header.source_id.to_le_bytes());
            head_bytes.push(header.ttl);
            head_bytes.extend_from_slice(&header._pad);

            self.send_frame(channel::ROUTING, &head_bytes, &req_bytes)
                .await?;
        } else {
            // DIRECT REQUEST
            self.send_frame(channel::APP, &[], &req_bytes).await?;
        }

        // Wait for response
        let resp_bytes = self.recv_frame().await?;

        // TODO: Handle routed response headers if necessary.
        // For MVP, router unwraps response and sends raw bytes back on the stream.

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

        self.stream
            .write_all(&len_bytes)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        self.stream
            .write_u8(channel)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        if !header.is_empty() {
            self.stream
                .write_all(header)
                .await
                .map_err(|_| cell_core::CellError::IoError)?;
        }
        self.stream
            .write_all(payload)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        Ok(())
    }

    async fn recv_frame(&mut self) -> Result<Vec<u8>, cell_core::CellError> {
        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|_| cell_core::CellError::ConnectionReset)?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        self.stream
            .read_exact(&mut buf)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;

        // Strip channel byte (first byte) logic is inside transport usually,
        // but here we are doing raw stream ops for the refactor.
        // Assuming the cell sends back raw response for APP channel.
        // If routed, the router sends back payload.

        // For simplicity in this heavy refactor, we assume response is just payload
        // The transport layer usually strips the channel byte.
        // Let's assume standard cell response format: [Chan][Data]
        if len > 0 {
            Ok(buf[1..].to_vec())
        } else {
            Ok(vec![])
        }
    }
}

async fn connect_with_retry(path: &std::path::Path, retry: bool) -> Result<UnixStream> {
    let max = if retry { 10 } else { 1 };
    for _ in 0..max {
        if let Ok(s) = UnixStream::connect(path).await {
            return Ok(s);
        }
        if retry {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    bail!("Connection failed to {:?}", path)
}
