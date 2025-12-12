// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use cell_model::config::CellInitConfig;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, Context, anyhow};
use cell_model::rkyv::Deserialize;

/// Client interface for the Cell System Daemon (Root).
/// Allows Cells to request infrastructure operations like spawning new cells.
pub struct System;

impl System {
    /// Request the Daemon to spawn a new Cell.
    /// 
    /// This sends a `MitosisRequest` to the Root process via the umbilical socket.
    /// It returns the socket path of the spawned cell.
    pub async fn spawn(cell_name: &str, config: Option<CellInitConfig>) -> Result<String> {
        let socket_dir = resolve_socket_dir();
        let umbilical = socket_dir.join("mitosis.sock");

        if !umbilical.exists() {
            return Err(anyhow!("System Daemon not found at {:?}. Is the Root running?", umbilical));
        }

        let mut stream = UnixStream::connect(&umbilical).await
            .context("Failed to connect to System Daemon")?;

        let req = MitosisRequest::Spawn {
            cell_name: cell_name.to_string(),
            config,
        };

        let req_bytes = cell_model::rkyv::to_bytes::<_, 256>(&req)?.into_vec();
        
        // Protocol: [Len u32][Body...]
        stream.write_all(&(req_bytes.len() as u32).to_le_bytes()).await?;
        stream.write_all(&req_bytes).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        
        let mut resp_buf = vec![0u8; len];
        stream.read_exact(&mut resp_buf).await?;

        let archived = cell_model::rkyv::check_archived_root::<MitosisResponse>(&resp_buf)
            .map_err(|e| anyhow!("Invalid system response: {:?}", e))?;
            
        let resp: MitosisResponse = archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap();

        match resp {
            MitosisResponse::Ok { socket_path } => Ok(socket_path),
            MitosisResponse::Denied { reason } => Err(anyhow!("Spawn denied: {}", reason)),
        }
    }
}