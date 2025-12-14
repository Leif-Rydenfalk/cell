// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod hardware;
pub mod health;
pub mod lan;
pub mod local;

// Re-export LanDiscovery for SDK convenience
pub use health::HealthChecker;
pub use lan::LanDiscovery;

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

        self.status.is_alive =
            self.status.local_latency.is_some() || self.status.lan_latency.is_some();
    }
}

pub struct Discovery;

impl Discovery {
    pub async fn scan() -> Vec<CellNode> {
        let lan_signals = lan::LanDiscovery::global().all().await;

        // Scan all search paths (System Scope is now default)
        let search_paths = get_search_paths();
        let mut local_sockets = Vec::new();

        for dir in search_paths {
            if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("sock") {
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            if stem != "mitosis" {
                                // Skip daemon socket
                                local_sockets.push((stem.to_string(), path));
                            }
                        }
                    }
                }
            }
        }

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
        for (name, path) in local_sockets {
            // Deduplicate if already found via LAN?
            // For now add them; higher layers or UI can filter.
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
    // 1. Env Override (Exact path force)
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return PathBuf::from(p);
    }

    // 2. Default Hierarchy
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let base = home.join(".cell/runtime");

    // 3. Scope Resolution (System vs Organism)
    // CHANGED: Default is now "system". "default" namespace is removed.
    let org = std::env::var("CELL_ORGANISM").unwrap_or_else(|_| "system".to_string());
    base.join(org)
}

/// Returns a list of directories to search for Cell sockets.
pub fn get_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let primary = resolve_socket_dir();
    paths.push(primary.clone());

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let system = home.join(".cell/runtime/system");

    // If we are in a custom organism (not system), we still check system as fallback/kernel
    if primary != system {
        paths.push(system);
    }

    paths
}
