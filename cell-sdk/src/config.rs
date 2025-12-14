// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::resolve_socket_dir;
use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CellConfig {
    pub node_id: u64,
    pub socket_dir: PathBuf,
    pub peers: Vec<String>,
    pub raft_storage_path: Option<PathBuf>,
}

impl CellConfig {
    pub fn from_env(cell_name: &str) -> Result<Self> {
        let node_id = env::var("CELL_NODE_ID")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<u64>()
            .context("CELL_NODE_ID must be a u64 integer")?;

        let socket_dir = resolve_socket_dir();

        let storage_path = if let Ok(p) = env::var("CELL_RAFT_PATH") {
            PathBuf::from(p)
        } else {
            socket_dir.join(format!("{}.wal", cell_name))
        };

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

    pub fn new(id: u64, peers: Vec<String>) -> Self {
        Self {
            node_id: id,
            socket_dir: resolve_socket_dir(),
            peers,
            raft_storage_path: None,
        }
    }
}
