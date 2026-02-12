// cells/ledger-v2/src/main.rs
// SPDX-License-Identifier: MIT
// Double-Entry Bookkeeping with Immutable Audit Log

use cell_sdk::*;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};

// === PROTOCOL ===

#[protein]
pub struct Transaction {
    pub reference: String,
    pub description: String,
    pub postings: Vec<Posting>,
}

#[protein]
pub struct Posting {
    pub account: String,
    pub amount: i64, // Positive = Debit, Negative = Credit (or vice versa depending on convention)
    pub asset: String,
}

#[protein]
pub struct EntryRecord {
    pub id: u64,
    pub tx: Transaction,
    pub timestamp: i64,
    pub prev_hash: Vec<u8>,
    pub hash: Vec<u8>,
}

#[protein]
pub struct BalanceQuery {
    pub account: String,
    pub asset: String,
}

// === SERVICE ===

struct LedgerState {
    // The immutable log
    entries: Vec<EntryRecord>,
    // Current state materialized view
    balances: HashMap<(String, String), i64>, // (Account, Asset) -> Balance
}

#[service]
#[derive(Clone)]
struct LedgerService {
    state: Arc<RwLock<LedgerState>>,
}

impl LedgerService {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(LedgerState {
                entries: Vec::new(),
                balances: HashMap::new(),
            })),
        }
    }
}

#[handler]
impl LedgerService {
    async fn record(&self, tx: Transaction) -> Result<u64> {
        // 1. Verify Double-Entry Constraint (Sum must be zero per asset)
        let mut sums: HashMap<String, i64> = HashMap::new();
        for p in &tx.postings {
            *sums.entry(p.asset.clone()).or_default() += p.amount;
        }

        for (asset, sum) in sums {
            if sum != 0 {
                bail!("Transaction unbalanced for asset {}: sum is {}", asset, sum);
            }
        }

        let mut state = self.state.write().await;
        
        // 2. Prepare Audit Record
        let prev_hash = state.entries.last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| vec![0u8; 32]); // Genesis hash

        let id = state.entries.len() as u64 + 1;
        
        // Compute Hash (Merkle Chain)
        let mut hasher = blake3::Hasher::new();
        hasher.update(&prev_hash);
        hasher.update(&id.to_le_bytes());
        hasher.update(tx.reference.as_bytes());
        // In real impl, hash full structure
        
        let hash = hasher.finalize().as_bytes().to_vec();

        // 3. Update State (Materialize)
        for p in &tx.postings {
            let key = (p.account.clone(), p.asset.clone());
            *state.balances.entry(key).or_default() += p.amount;
        }

        let record = EntryRecord {
            id,
            tx,
            timestamp: Utc::now().timestamp(),
            prev_hash,
            hash,
        };
        
        state.entries.push(record);
        
        tracing::info!("[Ledger] Recorded Tx #{}: {}", id, id);
        Ok(id)
    }

    async fn balance(&self, query: BalanceQuery) -> Result<i64> {
        let state = self.state.read().await;
        let bal = state.balances.get(&(query.account, query.asset)).copied().unwrap_or(0);
        Ok(bal)
    }

    async fn audit(&self, id: u64) -> Result<EntryRecord> {
        let state = self.state.read().await;
        state.entries.get((id - 1) as usize)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Entry not found"))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Ledger-V2] Financial Engine Active (Immutable Mode)");
    
    let service = LedgerService::new();
    service.serve("ledger-v2").await
}