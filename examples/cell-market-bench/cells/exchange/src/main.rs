use anyhow::Result;
use cell_sdk::*;

#[service]
#[derive(Clone)]
struct ExchangeService;

#[handler]
impl ExchangeService {
    // Fast path: pure throughput test
    async fn place_order(&self, _symbol: String, amount: u64, _side: u8) -> Result<u64> {
        // Minimal logic to burn a few cycles but mostly test serialization/transport
        Ok(amount)
    }

    async fn ping(&self, seq: u64) -> Result<u64> {
        Ok(seq)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Disable noisy logs for benchmark
    tracing_subscriber::fmt().with_env_filter("error").init();

    println!("--- EXCHANGE ONLINE ---");
    let service = ExchangeService;
    service.serve("exchange").await
}
