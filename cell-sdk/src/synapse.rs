// cell-sdk/src/synapse.rs
// SPDX-License-Identifier: MIT
// The Neural Connection. Polymorphic Transport.

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
use tokio::net::UnixStream;

// --- 1. The Abstraction ----------------------------------------------------

#[async_trait::async_trait]
trait RawTransport: Send + Sync {
    async fn send(&mut self, buf: &[u8]) -> Result<()>;
    async fn recv(&mut self, buf: &mut [u8]) -> Result<()>;
    async fn flush(&mut self) -> Result<()>;
}

struct PipeTransport {
    rx: File,
    tx: File,
}

#[async_trait::async_trait]
impl RawTransport for PipeTransport {
    async fn send(&mut self, buf: &[u8]) -> Result<()> {
        self.tx
            .write_all(buf)
            .await
            .map_err(|e| anyhow::Error::new(e))
    }
    async fn recv(&mut self, buf: &mut [u8]) -> Result<()> {
        self.rx
            .read_exact(buf)
            .await
            .map_err(|e| anyhow::Error::new(e))
            .map(|_| ())
    }
    async fn flush(&mut self) -> Result<()> {
        self.tx.flush().await.map_err(|e| anyhow::Error::new(e))
    }
}

struct SocketTransport {
    stream: UnixStream,
}

#[async_trait::async_trait]
impl RawTransport for SocketTransport {
    async fn send(&mut self, buf: &[u8]) -> Result<()> {
        self.stream
            .write_all(buf)
            .await
            .map_err(|e| anyhow::Error::new(e))
    }
    async fn recv(&mut self, buf: &mut [u8]) -> Result<()> {
        self.stream
            .read_exact(buf)
            .await
            .map_err(|e| anyhow::Error::new(e))
            .map(|_| ())
    }
    async fn flush(&mut self) -> Result<()> {
        self.stream.flush().await.map_err(|e| anyhow::Error::new(e))
    }
}

// Placeholder for future zero-copy implementation
struct ShmTransport {
    // ring: MmapRing
}
// impl RawTransport for ShmTransport ...

// --- 2. The Synapse (User Facing) ------------------------------------------

pub struct Synapse {
    transport: Box<dyn RawTransport>,
    routing_target: Option<u64>,
    request_id: AtomicU32,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = resolve_socket_dir();
        let neighbors_dir = socket_dir.join("neighbors");

        // STRATEGY 1: Direct Neighbor
        let direct_path = neighbors_dir.join(cell_name);
        if direct_path.exists() {
            if let Ok(transport) = connect_transport(&direct_path).await {
                return Ok(Self::new_direct(transport));
            }
            // Retry logic
            Self::try_autostart(cell_name).await?;
            // Backoff slightly
            tokio::time::sleep(Duration::from_millis(100)).await;
            let transport = connect_transport(&direct_path).await?;
            return Ok(Self::new_direct(transport));
        }

        // STRATEGY 2: Routed via Gateway
        let gateway_path = neighbors_dir.join("default");
        if gateway_path.exists() {
            let transport = connect_transport(&gateway_path)
                .await
                .context("Default gateway defined but unreachable")?;
            return Ok(Self::new_routed(transport, cell_name));
        }

        // STRATEGY 3: Autogenesis
        if Self::try_autostart(cell_name).await.is_ok() {
            if direct_path.exists() {
                let transport = connect_transport(&direct_path).await?;
                return Ok(Self::new_direct(transport));
            }
        }

        bail!("Cell '{}' unreachable.", cell_name);
    }

    fn new_direct(transport: Box<dyn RawTransport>) -> Self {
        Self {
            transport,
            routing_target: None,
            request_id: AtomicU32::new(0),
        }
    }

    fn new_routed(transport: Box<dyn RawTransport>, target_name: &str) -> Self {
        let hash = blake3::hash(target_name.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash.as_bytes()[..8]);
        let target_id = u64::from_le_bytes(bytes);

        Self {
            transport,
            routing_target: Some(target_id),
            request_id: AtomicU32::new(0),
        }
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
            // ROUTED
            let header = VesicleHeader {
                target_id: target,
                source_id: 0,
                ttl: 64,
                _pad: [0; 7],
            };

            let mut head_bytes = Vec::with_capacity(24);
            head_bytes.extend_from_slice(&header.target_id.to_le_bytes());
            head_bytes.extend_from_slice(&header.source_id.to_le_bytes());
            head_bytes.push(header.ttl);
            head_bytes.extend_from_slice(&header._pad);

            self.send_frame(channel::ROUTING, &head_bytes, &req_bytes)
                .await?;
        } else {
            // DIRECT
            self.send_frame(channel::APP, &[], &req_bytes).await?;
        }

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

        self.transport
            .send(&len_bytes)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        self.transport
            .send(&[channel])
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        if !header.is_empty() {
            self.transport
                .send(header)
                .await
                .map_err(|_| cell_core::CellError::IoError)?;
        }
        self.transport
            .send(payload)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        self.transport
            .flush()
            .await
            .map_err(|_| cell_core::CellError::IoError)?;
        Ok(())
    }

    async fn recv_frame(&mut self) -> Result<Vec<u8>, cell_core::CellError> {
        let mut len_buf = [0u8; 4];
        self.transport
            .recv(&mut len_buf)
            .await
            .map_err(|_| cell_core::CellError::ConnectionReset)?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        self.transport
            .recv(&mut buf)
            .await
            .map_err(|_| cell_core::CellError::IoError)?;

        if len > 0 {
            Ok(buf[1..].to_vec())
        } else {
            Ok(vec![])
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
                let instance = std::env::var("CELL_INSTANCE").unwrap_or_default();
                let mut cmd = std::process::Command::new("cell");
                cmd.args(&["run", "--path", path, "--instance", &instance, "--release"]);
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                cmd.spawn().context("Failed to spawn cell CLI")?;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        Ok(())
    }
}

// --- 3. The Auto-Detection Logic -------------------------------------------

async fn connect_transport(path: &Path) -> Result<Box<dyn RawTransport>> {
    // 1. Check Metadata
    let meta = std::fs::metadata(path).context("Failed to stat path")?;
    let file_type = meta.file_type();

    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        // CASE A: Socket -> UnixStream
        if file_type.is_socket() {
            let stream = UnixStream::connect(path).await?;
            return Ok(Box::new(SocketTransport { stream }));
        }

        // CASE B: FIFO -> Named Pipe Pair
        // If 'path' is a directory containing 'rx' and 'tx', we treat it as a Pipe Pair.
        // OR if 'path' itself is a FIFO? No, pipes are unidirectional, we need two.
        // Convention: The CLI creates a directory for the neighbor containing pipes.
        if file_type.is_dir() {
            let tx_path = path.join("tx");
            let rx_path = path.join("rx");

            // Note: We intentionally open TX then RX to match the CLI logic
            // OpenOptions usage handles the blocking behavior on FIFO open
            let tx = OpenOptions::new().write(true).open(&tx_path).await?;
            let rx = OpenOptions::new().read(true).open(&rx_path).await?;

            return Ok(Box::new(PipeTransport { rx, tx }));
        }

        // CASE C: SHM File (Future)
        // if file_type.is_file() {
        //     let mut file = File::open(path).await?;
        //     let mut magic = [0u8; 4];
        //     file.read_exact(&mut magic).await?;
        //     if &magic == b"CSHM" { ... }
        // }
    }

    bail!("Unsupported transport type at {:?}", path);
}
