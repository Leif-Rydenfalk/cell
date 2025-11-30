use anyhow::Result;
use cell_sdk as cell;
use cell_sdk::membrane::Membrane;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

struct ExchangeState {
    trade_count: AtomicU64,
    batch_ops: AtomicU64,
    bytes_received: AtomicU64,
}

#[cell::service]
#[derive(Clone)]
struct ExchangeService {
    state: Arc<ExchangeState>,
}

#[cell::handler]
impl ExchangeService {
    /// Standard lightweight operation
    async fn place_order(&self, _symbol: String, _amount: u64, _side: u8) -> Result<u64> {
        let id = self.state.trade_count.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    /// Optimized batch operation (High TPS test)
    async fn submit_batch(&self, count: u32) -> Result<u64> {
        let start = self.state.trade_count.fetch_add(count as u64, Ordering::Relaxed);
        self.state.batch_ops.fetch_add(1, Ordering::Relaxed);
        Ok(start + count as u64)
    }

    /// Payload absorption (Bandwidth test)
    /// Receives a byte vector and drops it, counting the bytes.
    async fn ingest_data(&self, data: Vec<u8>) -> Result<u64> {
        let len = data.len() as u64;
        self.state.bytes_received.fetch_add(len, Ordering::Relaxed);
        Ok(len)
    }

    async fn snapshot(&self) -> Result<u64> {
        Ok(self.state.trade_count.load(Ordering::Relaxed))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let state = Arc::new(ExchangeState {
        trade_count: AtomicU64::new(0),
        batch_ops: AtomicU64::new(0),
        bytes_received: AtomicU64::new(0),
    });

    let service = ExchangeService { state: state.clone() };

    // Background stats printer for the server side
    let state_monitor = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let ops = state_monitor.batch_ops.load(Ordering::Relaxed);
            let bytes = state_monitor.bytes_received.load(Ordering::Relaxed);
            println!("[Exchange Stats] Total Batches: {} | Total Bytes Ingested: {} MB", 
                ops, bytes / 1024 / 1024);
        }
    });

    println!("[Exchange] Online. Fingerprint: 0x{:x}", ExchangeService::SCHEMA_FINGERPRINT);
    
    Membrane::bind("exchange", move |vesicle| {
        let mut s = service.clone();
        async move {
            let bytes = s.handle_cell_message(vesicle.as_slice()).await?;
            Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
        }
    }, Some(ExchangeService::CELL_GENOME.to_string())).await
}