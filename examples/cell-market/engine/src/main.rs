use anyhow::{Result, bail};
use cell_sdk::{cell_remote, service, handler, protein};
use cell_sdk as cell;
use tracing::{info, error};
use std::sync::Arc;
use tokio::sync::Mutex;

cell_remote!(Ledger = "ledger");
cell_remote!(Consensus = "consensus-raft");

#[protein]
pub enum Side { Buy, Sell }

#[derive(Clone)]
struct EngineState {
    ledger: Arc<Mutex<Ledger::Client>>,
    consensus: Arc<Mutex<Consensus::Client>>,
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

        let mut client = self.state.ledger.lock().await;
        let result = client.lock_funds(user, asset, lock_amt).await?;

        match result {
            Ok(true) => {
                // Log to consensus
                let order_data = format!("ORDER:{}:{}:{}", user, price, amount).into_bytes();
                let cmd = Consensus::Command { data: order_data };
                
                let mut consensus = self.state.consensus.lock().await;
                let _ = consensus.propose(cmd).await?;
                
                info!("[Engine] Order logged to consensus");
                Ok(12345)
            },
            Ok(false) => bail!("Insufficient Funds"),
            Err(e) => bail!("Ledger Error: {}", e),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Engine] Online");

    let ledger_client = Ledger::connect().await?;
    let consensus_client = Consensus::connect().await?;

    let service = EngineService {
        state: EngineState {
            ledger: Arc::new(Mutex::new(ledger_client)),
            consensus: Arc::new(Mutex::new(consensus_client)),
        }
    };

    service.serve("engine").await
}