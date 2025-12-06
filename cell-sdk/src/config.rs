// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, Context};
use std::path::PathBuf;
use std::env;

#[derive(Debug, Clone)]
pub struct CellConfig {
    pub node_id: u64,
    pub socket_dir: PathBuf,
    pub peers: Vec<String>,
    pub raft_storage_path: Option<PathBuf>,
}

impl CellConfig {
    pub fn from_env(cell_name: &str) -> Result<Self> {
        // 1. Node Identity
        let node_id = env::var("CELL_NODE_ID")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<u64>()
            .context("CELL_NODE_ID must be a u64 integer")?;

        // 2. Directories
        let socket_dir = crate::resolve_socket_dir();
        
        let storage_path = if let Ok(p) = env::var("CELL_RAFT_PATH") {
            PathBuf::from(p)
        } else {
            socket_dir.join(format!("{}.wal", cell_name))
        };

        // 3. Topology
        let peers = env::var("CELL_PEERS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(Self {
            node_id,
            socket_dir,
            peers,
            raft_storage_path: Some(storage_path),
        })
    }

    /// For manual configuration (e.g. testing)
    pub fn new(id: u64, peers: Vec<String>) -> Self {
        Self {
            node_id: id,
            socket_dir: crate::resolve_socket_dir(),
            peers,
            raft_storage_path: None,
        }
    }
}