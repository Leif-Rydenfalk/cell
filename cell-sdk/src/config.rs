// SPDX-License-Identifier: MIT
// cell-sdk/src/config.rs

use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CellConfig {
    pub node_id: u64,
    pub peers: Vec<String>,
    pub raft_storage_path: Option<PathBuf>,
}

impl CellConfig {
    pub fn from_env(cell_name: &str) -> Result<Self> {
        let node_id = env::var("CELL_NODE_ID")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<u64>()
            .context("CELL_NODE_ID must be a u64 integer")?;

        // Storage is local to the cell's directory
        let cwd = env::current_dir()?;
        let storage_path = cwd.join(".cell/storage").join(format!("{}.wal", cell_name));

        let peers = env::var("CELL_PEERS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(Self {
            node_id,
            peers,
            raft_storage_path: Some(storage_path),
        })
    }
}
