use anyhow::Result;
use cell_sdk::*; // Imports service, handler macros

#[service]
#[derive(Clone)]
struct ExchangeService;

#[handler]
impl ExchangeService {
    // Defines the contract. Clients will see `exchange.place_order(...)`
    async fn place_order(&self, symbol: String, amount: u64, side: u8) -> Result<u64> {
        tracing::info!("Order received: {} {} (Side: {})", amount, symbol, side);
        Ok(amount)
    }

    async fn ping(&self, seq: u64) -> Result<u64> {
        Ok(seq)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    tracing::info!("--- EXCHANGE BOOT ---");
    
    let service = ExchangeService;
    // Blocks forever handling requests
    service.serve("exchange").await
}