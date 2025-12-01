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
    // Standard Handler (Deep Copy)
    // The macro sees `String` (owned), so it deserializes arguments from SHM -> Heap.
    async fn place_order(&self, _symbol: String, _amount: u64, _side: u8) -> Result<u64> {
        let id = self.state.trade_count.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    async fn submit_batch(&self, count: u32) -> Result<u64> {
        let start = self.state.trade_count.fetch_add(count as u64, Ordering::Relaxed);
        self.state.batch_ops.fetch_add(1, Ordering::Relaxed);
        Ok(start + count as u64)
    }

    // --- TRUE ZERO-COPY HANDLER ---
    // The macro sees `&Archived<Vec<u8>>` (Reference).
    // It passes the pointer from Shared Memory directly. No allocation!
    async fn ingest_data(&self, data: &Archived<Vec<u8>>) -> Result<u64> {
        // `data` is NOT a Vec<u8> on the heap. It is a view into the ring buffer.
        // It behaves exactly like a slice.
        let len = data.len() as u64;
        
        // We can read bytes without copying
        if len > 0 {
            let _first_byte = data[0]; 
        }

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

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let ops = state.batch_ops.load(Ordering::Relaxed);
            let bytes = state.bytes_received.load(Ordering::Relaxed);
            println!("[Exchange Stats] Total Batches: {} | Total Bytes Ingested: {} MB", 
                ops, bytes / 1024 / 1024);
        }
    });

    println!("[Exchange] Online. Fingerprint: 0x{:x}", ExchangeService::SCHEMA_FINGERPRINT);
    
    // The macro generated a strictly typed `serve` method for you
    service.serve("exchange").await?;
    
    Ok(())
}