use crate::protocol::{MitosisRequest, MitosisResponse};
use crate::vesicle::Vesicle;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub struct Synapse {
    stream: UnixStream,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));

        // 1. FAST PATH: Is it alive?
        if let Ok(stream) = UnixStream::connect(&socket_path).await {
            return Ok(Self { stream });
        }

        // 2. SLOW PATH: Tug the Umbilical Cord
        let umbilical_path = resolve_umbilical_path();

        let mut umbilical = UnixStream::connect(&umbilical_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to connect to Umbilical Cord at {:?}. Is the Root running?",
                    umbilical_path
                )
            })?;

        // Send Spawn Request
        let req = MitosisRequest::Spawn {
            cell_name: cell_name.into(),
        };
        let bytes = crate::rkyv::to_bytes::<_, 256>(&req)
            .map_err(|e| anyhow::anyhow!("Serialization failed: {}", e))?
            .into_vec();

        umbilical
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await?;
        umbilical.write_all(&bytes).await?;

        // Wait for Response
        let mut len_buf = [0u8; 4];
        umbilical.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        umbilical.read_exact(&mut buf).await?;

        let resp = crate::rkyv::from_bytes::<MitosisResponse>(&buf)
            .map_err(|e| anyhow::anyhow!("Deserialization failed: {:?}", e))?;

        match resp {
            MitosisResponse::Ok { .. } => {
                // 3. WAIT FOR GERMINATION
                // Retry loop to allow process startup time
                for _ in 0..50 {
                    if let Ok(stream) = UnixStream::connect(&socket_path).await {
                        return Ok(Self { stream });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                bail!(
                    "Cell '{}' spawned but failed to bind socket at {:?} in time.",
                    cell_name,
                    socket_path
                );
            }
            MitosisResponse::Denied { reason } => bail!("Mitosis Denied: {}", reason),
        }
    }

    pub async fn fire<
        T: crate::rkyv::Serialize<crate::rkyv::ser::serializers::AllocSerializer<1024>>,
    >(
        &mut self,
        payload: T,
    ) -> Result<Vesicle> {
        let bytes = crate::rkyv::to_bytes::<_, 1024>(&payload)
            .map_err(|e| anyhow::anyhow!("Serialization error: {}", e))?
            .into_vec();

        self.fire_bytes(bytes).await
    }

    pub async fn fire_bytes(&mut self, bytes: Vec<u8>) -> Result<Vesicle> {
        self.stream
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .await?;
        self.stream.write_all(&bytes).await?;

        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;

        Ok(Vesicle::wrap(buf))
    }
}

// --- Discovery Logic ---

fn resolve_socket_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }

    // Heuristic: Are we in a container?
    // Check if the Capsid-mounted paths exist.
    let container_socket_dir = std::path::Path::new("/tmp/cell");
    let container_umbilical = std::path::Path::new("/tmp/mitosis.sock");

    if container_socket_dir.exists() && container_umbilical.exists() {
        return container_socket_dir.to_path_buf();
    }

    // Fallback to Host Home
    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run");
    }

    PathBuf::from("/tmp/cell")
}

fn resolve_umbilical_path() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_UMBILICAL") {
        return PathBuf::from(p);
    }

    // Container Path (Priority if exists)
    let container_cord = std::path::Path::new("/tmp/mitosis.sock");
    if container_cord.exists() {
        return container_cord.to_path_buf();
    }

    // Host Path
    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run/mitosis.sock");
    }

    PathBuf::from("/tmp/mitosis.sock")
}
