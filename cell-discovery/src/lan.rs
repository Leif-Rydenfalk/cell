// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::hardware::HardwareCaps;

const MAX_CACHE_SIZE: usize = 10_000;

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct Signal {
    pub cell_name: String,
    pub instance_id: u64,
    pub ip: String,
    pub port: u16,
    pub timestamp: u64,
    pub hardware: HardwareCaps,
}

pub struct LanDiscovery {
    // Key: Cell Name -> Key: Instance ID -> Signal
    cache: Arc<RwLock<HashMap<String, HashMap<u64, Signal>>>>,
}

impl LanDiscovery {
    pub fn global() -> &'static Self {
        static INSTANCE: std::sync::OnceLock<LanDiscovery> = std::sync::OnceLock::new();
        INSTANCE.get_or_init(|| {
             let ld = Self {
                cache: Arc::new(RwLock::new(HashMap::new())),
            };
            ld.start_pruning();
            ld
        })
    }

    pub async fn update(&self, sig: Signal) {
        let mut cache = self.cache.write().await;
        
        // Pruning logic if too large (simplified for map-of-maps)
        if cache.len() >= MAX_CACHE_SIZE {
            cache.clear();
        }

        cache
            .entry(sig.cell_name.clone())
            .or_insert_with(HashMap::new)
            .insert(sig.instance_id, sig);
    }

    pub async fn all(&self) -> Vec<Signal> {
        let cache = self.cache.read().await;
        cache.values()
            .flat_map(|inner| inner.values())
            .cloned()
            .collect()
    }

    pub async fn find_any(&self, name: &str) -> Option<Signal> {
        let cache = self.cache.read().await;
        cache.get(name)
            .and_then(|inner| inner.values().next())
            .cloned()
    }

    pub async fn find_all(&self, name: &str) -> Vec<Signal> {
        let cache = self.cache.read().await;
        cache.get(name)
            .map(|inner| inner.values().cloned().collect())
            .unwrap_or_default()
    }

    fn start_pruning(&self) {
        let cache = self.cache.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                
                let mut guard = cache.write().await;
                // Prune stale signals inside the inner maps
                for inner in guard.values_mut() {
                    inner.retain(|_, v| now - v.timestamp < 60);
                }
                // Remove empty keys
                guard.retain(|_, v| !v.is_empty());
            }
        });
    }
}