use anyhow::{Result, bail};
use cell_sdk::{cell_remote, service, handler, protein};
use cell_sdk as cell;
use tracing::{info, error};
use std::sync::Arc;
use tokio::sync::Mutex;

// --- SYMBIOSIS ---
cell_remote!(Ledger = "ledger");

// --- LOCAL DNA ---
#[protein]
pub enum Side { Buy, Sell }

// --- LOGIC ---

// 1. Add state to hold the persistent connection
#[derive(Clone)]
struct EngineState {
    ledger: Arc<Mutex<Ledger::Client>>,
}

#[service]
#[derive(Clone)]
struct EngineService {
    state: EngineState,
}

#[handler]
impl EngineService {
    async fn place_order(&self, user: u64, _symbol: String, side: Side, price: u64, amount: u64) -> Result<u64> {
        let (asset, lock_amt) = match side {
            Side::Buy => (Ledger::Asset::USD, price * amount),
            Side::Sell => (Ledger::Asset::BTC, amount),
        };

        // 2. Lock the persistent connection instead of connecting
        let mut client = self.state.ledger.lock().await;
        
        // 3. Fire using the existing socket
        let result = client.lock_funds(user, asset, lock_amt).await?;

        match result {
            Ok(true) => {
                info!("[Engine] Funds Locked. Order Placed.");
                Ok(12345)
            },
            Ok(false) => {
                bail!("Insufficient Funds")
            },
            Err(e) => bail!("Ledger Logic Error: {}", e),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Engine] Online");

    // 4. Connect ONCE at startup
    info!("[Engine] Connecting to Ledger...");
    let ledger_client = Ledger::connect().await?;
    info!("[Engine] Connected.");

    let service = EngineService {
        state: EngineState {
            ledger: Arc::new(Mutex::new(ledger_client)),
        }
    };

    service.serve("engine").await
}