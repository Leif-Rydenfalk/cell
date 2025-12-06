// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod lan;
pub mod local;
pub mod health;

// Re-export LanDiscovery for SDK convenience
pub use lan::LanDiscovery;
pub use health::HealthChecker;

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CellNode {
    pub name: String,
    pub lan_address: Option<String>,
    pub local_socket: Option<PathBuf>,
    pub status: CellStatus,
}

#[derive(Debug, Clone, Default)]
pub struct CellStatus {
    pub local_latency: Option<Duration>,
    pub lan_latency: Option<Duration>,
    pub is_alive: bool,
}

impl CellNode {
    pub async fn probe(&mut self) {
        if let Some(path) = &self.local_socket {
            self.status.local_latency = local::probe_unix_socket(path).await;
        }
        
        self.status.is_alive = self.status.local_latency.is_some() || self.status.lan_latency.is_some();
    }
}

pub struct Discovery;

impl Discovery {
    pub async fn scan() -> Vec<CellNode> {
        let lan_map = lan::LanDiscovery::global().all().await;
        let local_names = local::scan_local_sockets().await;

        let mut map = std::collections::HashMap::new();

        for sig in lan_map {
            map.insert(
                sig.cell_name.clone(),
                CellNode {
                    name: sig.cell_name.clone(),
                    lan_address: Some(format!("{}:{}", sig.ip, sig.port)),
                    local_socket: None,
                    status: CellStatus::default(),
                },
            );
        }

        let socket_dir = resolve_socket_dir();
        for name in local_names {
            let path = socket_dir.join(format!("{}.sock", name));
            map.entry(name.clone())
                .and_modify(|node| node.local_socket = Some(path.clone()))
                .or_insert(CellNode {
                    name,
                    lan_address: None,
                    local_socket: Some(path),
                    status: CellStatus::default(),
                });
        }

        let mut list: Vec<CellNode> = map.into_values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

pub fn resolve_socket_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }
    let container_dir = std::path::Path::new("/tmp/cell");
    if container_dir.exists() {
        return container_dir.to_path_buf();
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run");
    }
    PathBuf::from("/tmp/cell")
}