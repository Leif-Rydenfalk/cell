use anyhow::Result;
use cell_sdk::{service, handler, protein};
use cell_sdk as cell;
use dashmap::DashMap;
use std::sync::Arc;
use tracing::info;

// --- DNA ---
// Note: No manual derives needed! #[protein] handles it all.

#[protein]
pub enum Asset { USD, BTC }

// --- LOGIC ---

struct LedgerState {
    accounts: DashMap<u64, DashMap<String, u64>>,
}

#[service]
#[derive(Clone)]
struct LedgerService {
    state: Arc<LedgerState>,
}

#[handler]
impl LedgerService {
    async fn deposit(&self, user: u64, asset: Asset, amount: u64) -> Result<u64> {
        let key = format!("{:?}", asset);
        let user_map = self.state.accounts.entry(user).or_insert_with(DashMap::new);
        let mut bal = user_map.entry(key).or_insert(0);
        *bal += amount;
        info!("[Ledger] Deposit: User {} +{} {:?}", user, amount, asset);
        Ok(*bal)
    }

    async fn lock_funds(&self, user: u64, asset: Asset, amount: u64) -> Result<bool> {
        let key = format!("{:?}", asset);
        if let Some(mut user_map) = self.state.accounts.get_mut(&user) {
            if let Some(mut bal) = user_map.get_mut(&key) {
                if *bal >= amount {
                    *bal -= amount;
                    info!("[Ledger] Locked: User {} -{} {:?}", user, amount, asset);
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Ledger] Online");
    let service = LedgerService { state: Arc::new(LedgerState { accounts: DashMap::new() }) };
    service.serve("ledger").await
}