use anyhow::{Result, bail};
use cell_sdk::{cell_remote, service, handler, protein};
use cell_sdk as cell;
use tracing::{info};
use std::sync::Arc;
use tokio::sync::Mutex;

cell_remote!(Ledger = "ledger");
cell_remote!(Consensus = "consensus"); // Changed from "consensus-raft" to "consensus" to match DNA

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
        
        // Ledger returns Result<bool, CellError> (unwrapped by macro from Result<Result<bool, AppError>>)
        // Let's assume the macro fix is applied, so it returns Result<bool, CellError>
        let result = client.lock_funds(user, asset, lock_amt).await;

        match result {
            Ok(true) => {
                // Log to consensus
                let order_data = format!("ORDER:{}:{}:{}", user, price, amount).into_bytes();
                let cmd = Consensus::Command { data: order_data };
                
                let mut consensus = self.state.consensus.lock().await;
                // Propose returns Result<u64, CellError> (or ProposeResult?)
                // Consensus handler returns Result<ProposeResult>
                // So client returns Result<ProposeResult, CellError>
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

    // Hydrate Identity (Critical for 'stem cell' behavior if this was being deployed)
    // But this is an example app.
    
    // Connect to dependencies
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