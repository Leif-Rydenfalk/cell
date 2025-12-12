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
    pub instance_id: u64,
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
        let lan_signals = lan::LanDiscovery::global().all().await;
        let local_names = local::scan_local_sockets().await;

        let mut nodes = Vec::new();

        // Add LAN nodes
        for sig in lan_signals {
            nodes.push(CellNode {
                name: sig.cell_name,
                instance_id: sig.instance_id,
                lan_address: Some(format!("{}:{}", sig.ip, sig.port)),
                local_socket: None,
                status: CellStatus::default(),
            });
        }

        // Add Local nodes
        let socket_dir = resolve_socket_dir();
        for name in local_names {
            let path = socket_dir.join(format!("{}.sock", name));
            
            nodes.push(CellNode {
                name,
                instance_id: 0, 
                lan_address: None,
                local_socket: Some(path),
                status: CellStatus::default(),
            });
        }

        nodes.sort_by(|a, b| a.name.cmp(&b.name).then(a.instance_id.cmp(&b.instance_id)));
        nodes
    }
}

pub fn resolve_socket_dir() -> PathBuf {
    // 1. Env Override (CI/Test)
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }

    // 2. Default Hierarchy
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let base = home.join(".cell/runtime");

    // 3. Scope Resolution (System vs Organism)
    if let Ok(org) = std::env::var("CELL_ORGANISM") {
        base.join(org)
    } else {
        base.join("system")
    }
}