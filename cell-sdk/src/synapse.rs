use crate::protocol::{MitosisRequest, MitosisResponse};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub struct Synapse {
    stream: UnixStream,
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let socket_dir = std::env::var("CELL_SOCKET_DIR").unwrap_or("/tmp/cell".into());
        let socket_path = PathBuf::from(socket_dir).join(format!("{}.sock", cell_name));

        // 1. FAST PATH: Is it alive?
        if let Ok(stream) = UnixStream::connect(&socket_path).await {
            return Ok(Self { stream });
        }

        // 2. SLOW PATH: Tug the Umbilical Cord
        let umbilical_path =
            std::env::var("CELL_UMBILICAL").unwrap_or("/tmp/cell/mitosis.sock".into());

        // We connect to the Root to ask for a spawn
        let mut umbilical = UnixStream::connect(&umbilical_path)
            .await
            .context("Cell is dead and Umbilical Cord is severed (Root process is gone).")?;

        // Send Spawn Request
        let req = MitosisRequest::Spawn {
            cell_name: cell_name.into(),
        };
        let bytes = crate::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
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

        let resp = crate::rkyv::from_bytes::<MitosisResponse>(&buf)?;

        match resp {
            MitosisResponse::Ok { .. } => {
                // 3. WAIT FOR GERMINATION
                // The process is launched, but socket might take milliseconds to bind
                for _ in 0..20 {
                    // Try for 1 second (20 * 50ms)
                    if let Ok(stream) = UnixStream::connect(&socket_path).await {
                        return Ok(Self { stream });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                bail!("Cell spawned but failed to bind socket in time.");
            }
            MitosisResponse::Denied { reason } => bail!("Mitosis Denied: {}", reason),
        }
    }
}
