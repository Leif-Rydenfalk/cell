// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

// This logic is partially shared/moved from cell-axon/pheromones.rs 
// Ideally cell-axon depends on cell-discovery, or vice-versa.
// For now, we define the Signal structure here and the LanDiscovery registry.
// The UDP socket listening logic remains in cell-axon for Pheromones but feeds into this registry.

use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

const MAX_CACHE_SIZE: usize = 10_000;

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct Signal {
    pub cell_name: String,
    pub ip: String,
    pub port: u16,
    pub timestamp: u64,
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
            // Simple pruning: remove stale or random if needed.
            // For brevity, we just clear if full or impl more complex logic.
            if cache.len() > MAX_CACHE_SIZE {
                cache.clear();
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
}