// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use cell_model::protocol::{MitosisRequest, MitosisResponse};
use cell_model::config::CellInitConfig;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, Context, anyhow};
use cell_model::rkyv::Deserialize;

#[cfg(feature = "process")]
use std::sync::Arc;
#[cfg(feature = "process")]
use tokio::sync::OnceCell;
#[cfg(feature = "process")]
use cell_process::MyceliumRoot;

#[cfg(feature = "process")]
static ROOT: OnceCell<Arc<MyceliumRoot>> = OnceCell::const_new();

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

    /// Bootstraps a complete, isolated local Cell environment (Root + Nucleus + Axon) 
    /// within the current process. 
    /// 
    /// This is designed for integration testing, benchmarks, or local development environments
    /// where a full system daemon is required without external setup.
    /// 
    /// # Side Effects
    /// - Sets environment variables (`CELL_SOCKET_DIR`, `CELL_HOME`, etc.)
    /// - Creates directories
    /// - Spawns background tasks for the Daemon
    #[cfg(feature = "process")]
    pub async fn ignite_local_cluster() -> Result<()> {
        ROOT.get_or_init(|| async {
            // 1. Configure isolated environment
            // Uses a target subdirectory to avoid polluting the user's actual home
            let mut target_dir = std::env::current_dir().unwrap();
            target_dir.push("target");
            target_dir.push("cell-cluster");
            
            if target_dir.exists() {
                let _ = std::fs::remove_dir_all(&target_dir);
            }
            std::fs::create_dir_all(&target_dir).unwrap();

            std::env::set_var("CELL_SOCKET_DIR", target_dir.to_str().unwrap());
            std::env::set_var("CELL_DISABLE_SHM", "0");
            std::env::set_var("CELL_NODE_ID", "100"); // Root ID
            
            let fake_home = target_dir.join("home");
            std::fs::create_dir_all(&fake_home).unwrap();
            std::env::set_var("HOME", fake_home.to_str().unwrap());

            // 2. Initialize Logging (if not already)
            let _ = tracing_subscriber::fmt()
                .with_env_filter("info")
                .with_test_writer()
                .try_init();

            // 3. Ignite Root Daemon
            let root = MyceliumRoot::ignite().await.expect("Failed to start Mycelium Root");
            
            // 4. Spawn Core Services via System API
            // We do this in a loop to allow the socket to come up
            for _ in 0..50 {
                if Self::spawn("nucleus", None).await.is_ok() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            
            Self::spawn("axon", None).await.expect("Failed to spawn Axon");

            // Allow discovery to settle
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            Arc::new(root)
        }).await;
        
        Ok(())
    }
}