// cells/dht/src/main.rs
// SPDX-License-Identifier: MIT
// Kademlia-inspired DHT for global state

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use sha1::{Sha1, Digest};

// === PROTOCOL ===

#[protein]
pub struct DhtStore {
    pub key: String,
    pub value: Vec<u8>,
    pub ttl_secs: u64,
}

#[protein]
pub struct DhtGet {
    pub key: String,
}

#[protein]
pub struct DhtValue {
    pub value: Option<Vec<u8>>,
    pub found_on: String,
}

// === SERVICE ===

struct Entry {
    value: Vec<u8>,
    expires_at: std::time::Instant,
}

pub struct DhtService {
    // In-memory storage for this node's shard
    storage: Arc<RwLock<HashMap<String, Entry>>>,
}

impl DhtService {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[allow(dead_code)]
    fn hash_key(key: &str) -> String {
        let mut hasher = Sha1::new();
        hasher.update(key.as_bytes());
        hex::encode(hasher.finalize())
    }
}

#[handler]
impl DhtService {
    pub async fn put(&self, req: DhtStore) -> Result<bool> {
        // In real Kademlia: Determine distance, route to closest nodes.
        // Here: Single cell acting as a DHT node (or simple sharded store).
        let mut store = self.storage.write().await;
        
        store.insert(req.key.clone(), Entry {
            value: req.value,
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(req.ttl_secs),
        });
        
        tracing::debug!("[DHT] Stored '{}'", req.key);
        Ok(true)
    }

    pub async fn get(&self, req: DhtGet) -> Result<DhtValue> {
        let store = self.storage.read().await;
        
        // Check expiration
        if let Some(entry) = store.get(&req.key) {
            if entry.expires_at > std::time::Instant::now() {
                return Ok(DhtValue {
                    value: Some(entry.value.clone()),
                    found_on: "local".to_string(),
                });
            }
        }

        Ok(DhtValue {
            value: None,
            found_on: "".to_string(),
        })
    }

    pub async fn stats(&self) -> Result<String> {
        let store = self.storage.read().await;
        Ok(format!("Keys stored: {}", store.len()))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let service = DhtService::new();
    tracing::info!("[DHT] Distributed Hash Table Node Active");
    service.serve("dht").await
}