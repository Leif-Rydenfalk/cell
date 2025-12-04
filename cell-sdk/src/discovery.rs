// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

//! Global Discovery (LAN + Local Sockets)

#![cfg(feature = "axon")]

use crate::pheromones::Signal;
use crate::protocol::GENOME_REQUEST;
use crate::resolve_socket_dir;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

// Security Fix #5: Bound Discovery Cache
const MAX_CACHE_SIZE: usize = 10_000;

// --- Data Structures ---

#[derive(Debug, Clone)]
pub struct CellNode {
    pub name: String,
    pub lan_address: Option<String>, // ip:port
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
        // Probe Local Socket
        if let Some(path) = &self.local_socket {
            self.status.local_latency = probe_unix_socket(path).await;
        }

        // Probe LAN
        if let Some(addr) = &self.lan_address {
            self.status.lan_latency = probe_lan_address(addr).await;
        }

        self.status.is_alive =
            self.status.local_latency.is_some() || self.status.lan_latency.is_some();
    }
}

async fn probe_unix_socket(path: &PathBuf) -> Option<Duration> {
    let start = Instant::now();
    let mut stream = tokio::net::UnixStream::connect(path).await.ok()?;

    // Send GENOME_REQUEST
    let req_len = GENOME_REQUEST.len() as u32;
    stream.write_all(&req_len.to_le_bytes()).await.ok()?;
    stream.write_all(GENOME_REQUEST).await.ok()?;

    // Read Response Header
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.ok()?;

    Some(start.elapsed())
}

async fn probe_lan_address(addr: &str) -> Option<Duration> {
    let start = Instant::now();

    // Connect
    let conn = crate::axon::AxonClient::connect_exact(addr).await.ok()??;

    // Open stream
    let (mut send, mut recv) = conn.open_bi().await.ok()?;

    // Send GENOME_REQUEST
    let req_len = GENOME_REQUEST.len() as u32;
    send.write_all(&req_len.to_le_bytes()).await.ok()?;
    send.write_all(GENOME_REQUEST).await.ok()?;
    send.finish().await.ok()?;

    // Read Response Header
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await.ok()?;

    Some(start.elapsed())
}

// --- LAN Cache ---

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
        let mut cache = self.cache.write().await;
        
        // Security Fix #5: Eviction to prevent unbounded growth
        if cache.len() >= MAX_CACHE_SIZE {
            // Simple eviction strategy: Remove oldest entries
            // Since HashMap doesn't track insertion order, we sort by timestamp
            let mut entries: Vec<_> = cache.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            entries.sort_by_key(|(_, s)| s.timestamp);
            
            // Remove bottom 10%
            for (name, _) in entries.iter().take(MAX_CACHE_SIZE / 10) {
                cache.remove(name);
            }
        }
        
        cache.insert(sig.cell_name.clone(), sig);
    }

    pub async fn all(&self) -> Vec<Signal> {
        self.cache.read().await.values().cloned().collect()
    }

    pub async fn find(&self, name: &str) -> Option<Signal> {
        self.cache.read().await.get(name).cloned()
    }

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

// --- Unified Discovery ---

pub struct Discovery;

impl Discovery {
    pub async fn scan() -> Vec<CellNode> {
        // 1. Snapshot LAN Cache
        let lan_map = LanDiscovery::global().cache.read().await.clone();

        // 2. Scan Local Sockets
        let local_names = scan_local_sockets().await;

        // 3. Merge (Name is the unique identity)
        let mut map: HashMap<String, CellNode> = HashMap::new();

        // Populate from LAN
        for (name, sig) in lan_map {
            map.insert(
                name.clone(),
                CellNode {
                    name,
                    lan_address: Some(format!("{}:{}", sig.ip, sig.port)),
                    local_socket: None,
                    status: CellStatus::default(),
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
                    status: CellStatus::default(),
                });
        }

        // 4. Merge Manual Peer (Robustness Fallback)
        if let Ok(peer) = std::env::var("CELL_PEER") {
            // CELL_PEER format is usually ip:port or ip:port:name?
            // Usually just ip:port, but discovery needs a name.
            // We treat CELL_PEER as a generic fallback.
            // If the user provided CELL_PEER, they likely want to see it.
            // Without a name, we can't key it easily, but let's try to probe it or list it as "manual"
            // For now, let's assume if it connects, we might get name from genome?
            // Ignoring for now to keep scan() simple and fast.
            // But if we have a name associated (e.g. from args), we could add it.
        }

        let mut list: Vec<CellNode> = map.into_values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }
}

async fn scan_local_sockets() -> Vec<String> {
    let mut names = vec![];
    let path = resolve_socket_dir();
    if !path.exists() {
        return names;
    }

    if let Ok(mut entries) = tokio::fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "sock" {
                    if let Some(stem) = path.file_stem() {
                        names.push(stem.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    names
}