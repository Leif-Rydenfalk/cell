// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

//! Global Discovery (LAN + Local Sockets)

#![cfg(feature = "axon")]

use crate::pheromones::Signal;
use crate::resolve_socket_dir;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

// --- Data Structures ---

#[derive(Debug, Clone)]
pub struct CellNode {
    pub name: String,
    pub lan_address: Option<String>, // ip:port
    pub local_socket: Option<PathBuf>,
}

impl CellNode {
    pub fn is_hybrid(&self) -> bool {
        self.lan_address.is_some() && self.local_socket.is_some()
    }
}

// --- LAN Cache (Passive UDP) ---

pub struct LanDiscovery {
    cache: Arc<RwLock<HashMap<String, Signal>>>,
}

impl LanDiscovery {
    pub fn global() -> &'static Self {
        static INSTANCE: std::sync::OnceLock<LanDiscovery> = std::sync::OnceLock::new();
        INSTANCE.get_or_init(|| Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn update(&self, sig: Signal) {
        self.cache.write().await.insert(sig.cell_name.clone(), sig);
    }

    pub async fn all(&self) -> Vec<Signal> {
        self.cache.read().await.values().cloned().collect()
    }

    pub async fn find(&self, name: &str) -> Option<Signal> {
        self.cache.read().await.get(name).cloned()
    }

    /// Start background task to prune stale entries
    pub fn start_pruning(max_age_secs: u64) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Self::global().prune_stale(max_age_secs).await;
            }
        });
    }

    pub async fn prune_stale(&self, max_age_secs: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut cache = self.cache.write().await;
        cache.retain(|_, sig| now - sig.timestamp < max_age_secs);
    }
}

// --- Unified Discovery (FS + LAN) ---

pub struct Discovery;

impl Discovery {
    /// Scans both the LAN cache and the local filesystem for Cells
    pub async fn scan() -> Vec<CellNode> {
        // 1. Snapshot LAN Cache
        let lan_map = LanDiscovery::global().cache.read().await.clone();

        // 2. Scan Local Sockets
        let local_names = scan_local_sockets().await;

        // 3. Merge results
        let mut map: HashMap<String, CellNode> = HashMap::new();

        // Populate from LAN
        for (name, sig) in lan_map {
            map.insert(
                name.clone(),
                CellNode {
                    name,
                    lan_address: Some(format!("{}:{}", sig.ip, sig.port)),
                    local_socket: None,
                },
            );
        }

        // Merge Local Sockets
        let socket_dir = resolve_socket_dir();
        for name in local_names {
            let path = socket_dir.join(format!("{}.sock", name));

            map.entry(name.clone())
                .and_modify(|node| node.local_socket = Some(path.clone()))
                .or_insert(CellNode {
                    name,
                    lan_address: None,
                    local_socket: Some(path),
                });
        }

        let mut list: Vec<CellNode> = map.into_values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

async fn scan_local_sockets() -> Vec<String> {
    let mut names = vec![];
    let path = resolve_socket_dir();

    // It's possible the dir doesn't exist if no local cells ever ran
    if !path.exists() {
        return names;
    }

    if let Ok(mut entries) = tokio::fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            // Look for .sock files
            if let Some(ext) = path.extension() {
                if ext == "sock" {
                    // But ignore .lock files or other artifacts,
                    // though .sock usually implies the socket itself.
                    if let Some(stem) = path.file_stem() {
                        names.push(stem.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    names
}
