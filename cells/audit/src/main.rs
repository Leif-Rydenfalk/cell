// cells/audit/src/main.rs
// SPDX-License-Identifier: MIT
// Compliance Logging & Tamper-Proof Records

use cell_sdk::*;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct AuditEvent {
    pub actor: String,
    pub action: String,
    pub resource: String,
    pub outcome: String, // Success/Failure
    pub metadata: String,
    pub timestamp: u64,
}

#[protein]
pub struct SignedEvent {
    pub id: u64,
    pub event: AuditEvent,
    pub prev_hash: Vec<u8>,
    pub hash: Vec<u8>,
}

#[protein]
pub struct Query {
    pub actor: Option<String>,
    pub limit: u32,
}

struct AuditState {
    chain: Vec<SignedEvent>,
}

#[service]
#[derive(Clone)]
struct AuditService {
    state: Arc<RwLock<AuditState>>,
}

impl AuditService {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(AuditState {
                chain: Vec::new(),
            })),
        }
    }
}

#[handler]
impl AuditService {
    async fn log(&self, event: AuditEvent) -> Result<u64> {
        let mut state = self.state.write().await;
        
        let prev_hash = state.chain.last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| vec![0u8; 32]);
            
        let id = state.chain.len() as u64 + 1;
        
        // Merkle Chain Hashing
        let mut hasher = blake3::Hasher::new();
        hasher.update(&prev_hash);
        hasher.update(&id.to_le_bytes());
        hasher.update(event.actor.as_bytes());
        hasher.update(event.action.as_bytes());
        hasher.update(event.timestamp.to_le_bytes());
        let hash = hasher.finalize().as_bytes().to_vec();
        
        let signed = SignedEvent {
            id,
            event,
            prev_hash,
            hash,
        };
        
        state.chain.push(signed);
        Ok(id)
    }

    async fn query(&self, q: Query) -> Result<Vec<SignedEvent>> {
        let state = self.state.read().await;
        let iter = state.chain.iter().rev();
        
        let filtered = if let Some(actor) = q.actor {
            iter.filter(|e| e.event.actor == actor).take(q.limit as usize).cloned().collect()
        } else {
            iter.take(q.limit as usize).cloned().collect()
        };
        
        Ok(filtered)
    }

    async fn verify(&self) -> Result<bool> {
        let state = self.state.read().await;
        let mut prev = vec![0u8; 32];
        
        for entry in &state.chain {
            if entry.prev_hash != prev {
                return Ok(false);
            }
            
            let mut hasher = blake3::Hasher::new();
            hasher.update(&prev);
            hasher.update(&entry.id.to_le_bytes());
            hasher.update(entry.event.actor.as_bytes());
            hasher.update(entry.event.action.as_bytes());
            hasher.update(entry.event.timestamp.to_le_bytes());
            let computed = hasher.finalize().as_bytes().to_vec();
            
            if computed != entry.hash {
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
    tracing::info!("[Audit] Compliance Logger Active");
    let service = AuditService::new();
    service.serve("audit").await
}