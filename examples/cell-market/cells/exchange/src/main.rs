use anyhow::Result;
use cell_sdk as cell;
use cell_sdk::rkyv::Archived;
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
    async fn place_order(&self, _symbol: String, _amount: u64, _side: u8) -> Result<u64> {
        Ok(self.state.trade_count.fetch_add(1, Ordering::Relaxed))
    }

    async fn submit_batch(&self, count: u32) -> Result<u64> {
        // Process batch item-by-item
        let mut executed = 0;
        for _ in 0..count {
            self.state.trade_count.fetch_add(1, Ordering::Relaxed);
            executed += 1;
        }

        self.state.batch_ops.fetch_add(1, Ordering::Relaxed);
        Ok(self.state.trade_count.load(Ordering::Relaxed))
    }

    async fn ingest_data(&self, data: &Archived<Vec<u8>>) -> Result<u64> {
        let len = data.len() as u64;
        if len > 0 { 
            let _ = data[0]; 
        }
        self.state.bytes_received.fetch_add(len, Ordering::Relaxed);
        Ok(len)
    }
    
    async fn ping(&self, seq: u64) -> Result<u64> {
        Ok(seq)
    }

    async fn snapshot(&self) -> Result<u64> {
        Ok(self.state.trade_count.load(Ordering::Relaxed))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Enable LAN mode automatically
    std::env::set_var("CELL_LAN", "1");
    
    let state = Arc::new(ExchangeState {
        trade_count: AtomicU64::new(0),
        batch_ops: AtomicU64::new(0),
        bytes_received: AtomicU64::new(0),
    });

    let service = ExchangeService { state: state.clone() };

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║           CELL EXCHANGE - MULTI-INTERFACE MODE           ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("[Exchange] Fingerprint: 0x{:x}", ExchangeService::SCHEMA_FINGERPRINT);
    println!("[Exchange] Starting server with automatic LAN discovery...");
    println!();
    
    service.serve("exchange").await?;
    
    Ok(())
}