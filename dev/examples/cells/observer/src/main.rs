// cells/observer/src/main.rs
// SPDX-License-Identifier: MIT
// Tamper-Evident Observability Bus

use cell_sdk::*;
use anyhow::{Result};
use std::sync::Arc;
use tokio::sync::RwLock;

// === PROTOCOL ===

#[protein]
pub struct TelemetrySpan {
    pub trace_id: String,
    pub span_id: String,
    pub service: String,
    pub name: String,
    pub duration_us: u64,
    pub tags: Vec<(String, String)>,
}

#[protein]
pub struct LogEntry {
    pub hash: String,
    pub prev_hash: String,
    pub span: TelemetrySpan,
}

// === SERVICE ===

struct ObserverState {
    logs: Vec<LogEntry>,
    last_hash: String,
}

#[service]
#[derive(Clone)]
struct ObserverService {
    state: Arc<RwLock<ObserverState>>,
}

impl ObserverService {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(ObserverState {
                logs: Vec::new(),
                last_hash: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            })),
        }
    }
}

#[handler]
impl ObserverService {
    async fn emit(&self, span: TelemetrySpan) -> Result<String> {
        let mut state = self.state.write().await;
        
        // 1. Canonicalize
        let json = serde_json::to_string(&span).unwrap_or_default();
        
        // 2. Chain Hash
        let mut hasher = blake3::Hasher::new();
        hasher.update(state.last_hash.as_bytes());
        hasher.update(json.as_bytes());
        
        let hash = hasher.finalize().to_hex().to_string();
        
        let entry = LogEntry {
            hash: hash.clone(),
            prev_hash: state.last_hash.clone(),
            span,
        };
        
        // 3. Store
        state.logs.push(entry);
        state.last_hash = hash.clone();
        
        // In real world: Flush to Grafana/Loki/Elastic
        // tracing::info!("[Observer] Ingested trace: {}", hash);
        
        Ok(hash)
    }

    async fn tail(&self, limit: u32) -> Result<Vec<LogEntry>> {
        let state = self.state.read().await;
        let start = state.logs.len().saturating_sub(limit as usize);
        Ok(state.logs[start..].to_vec())
    }

    async fn verify_chain(&self) -> Result<bool> {
        let state = self.state.read().await;
        
        let mut prev = "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        
        for entry in &state.logs {
            if entry.prev_hash != prev {
                tracing::error!("Broken chain at {}", entry.hash);
                return Ok(false);
            }
            
            let json = serde_json::to_string(&entry.span).unwrap_or_default();
            let mut hasher = blake3::Hasher::new();
            hasher.update(prev.as_bytes());
            hasher.update(json.as_bytes());
            let computed = hasher.finalize().to_hex().to_string();
            
            if computed != entry.hash {
                tracing::error!("Hash mismatch at {}", entry.hash);
                return Ok(false);
            }
            
            prev = entry.hash.clone();
        }
        
        Ok(true)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Observer] Telemetry Bus Active");
    
    let service = ObserverService::new();
    service.serve("observer").await
}