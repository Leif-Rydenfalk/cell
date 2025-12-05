// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::pheromones::Signal;
use cell_model::protocol::GENOME_REQUEST;
use cell_model::resolve_socket_dir;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

const MAX_CACHE_SIZE: usize = 10_000;

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
            self.status.local_latency = probe_unix_socket(path).await;
        }

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

    let req_len = GENOME_REQUEST.len() as u32;
    stream.write_all(&req_len.to_le_bytes()).await.ok()?;
    stream.write_all(GENOME_REQUEST).await.ok()?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.ok()?;

    Some(start.elapsed())
}

async fn probe_lan_address(addr: &str) -> Option<Duration> {
    let start = Instant::now();

    let conn = crate::axon::AxonClient::connect_exact(addr).await.ok()??;

    let (mut send, mut recv) = conn.open_bi().await.ok()?;

    let req_len = GENOME_REQUEST.len() as u32;
    send.write_all(&req_len.to_le_bytes()).await.ok()?;
    send.write_all(GENOME_REQUEST).await.ok()?;
    send.finish().await.ok()?;

    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await.ok()?;

    Some(start.elapsed())
}

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
        
        if cache.len() >= MAX_CACHE_SIZE {
            let mut entries: Vec<_> = cache.iter().map(|(k, v)| (k.clone(), v.timestamp)).collect();
            entries.sort_by_key(|(_, ts)| *ts);
            
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

pub struct Discovery;

impl Discovery {
    pub async fn scan() -> Vec<CellNode> {
        let lan_map = LanDiscovery::global().cache.read().await.clone();
        let local_names = scan_local_sockets().await;

        let mut map: HashMap<String, CellNode> = HashMap::new();

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