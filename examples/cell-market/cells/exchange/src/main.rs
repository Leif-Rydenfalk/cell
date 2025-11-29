use anyhow::Result;
use cell_sdk::{Membrane, protein};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[protein]
pub enum MarketProtocol {
    PlaceOrder {
        symbol: String,
        amount: u64,
        side: u8,
    },
    SubmitBatch {
        count: u32,
    },
    SnapshotRequest,
    OrderAck {
        id: u64,
    },
    SnapshotResponse {
        total_trades: u64,
    },
}

struct ExchangeState {
    trade_count: AtomicU64,
    batch_ops: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let state = Arc::new(ExchangeState {
        trade_count: AtomicU64::new(0),
        batch_ops: AtomicU64::new(0),
    });

    println!("[Exchange] Online. Fingerprint: {:x}", MarketProtocol::SCHEMA_FINGERPRINT);
    println!("[Exchange] Ready for benchmarking (Logs dampened).");

    Membrane::bind("exchange", move |vesicle| {
        let state = state.clone();
        async move {
            let msg = cell_sdk::rkyv::from_bytes::<MarketProtocol>(vesicle.as_slice())
                .map_err(|e| anyhow::anyhow!("Invalid Protein: {:?}", e))?;

            match msg {
                MarketProtocol::SubmitBatch { count } => {
                    // Fast path logic
                    let start = state.trade_count.fetch_add(count as u64, Ordering::Relaxed);
                    let ops = state.batch_ops.fetch_add(1, Ordering::Relaxed);
                    
                    // Log only every 10,000 requests to avoid I/O bottleneck
                    if ops % 10_000 == 0 {
                         println!("[Exchange] Processed {} batches (Total trades: {})", ops, start + count as u64);
                    }
                    
                    let ack = MarketProtocol::OrderAck { id: start + count as u64 };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&ack)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                MarketProtocol::PlaceOrder { .. } => {
                    let id = state.trade_count.fetch_add(1, Ordering::Relaxed);
                    let ack = MarketProtocol::OrderAck { id };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&ack)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                MarketProtocol::SnapshotRequest => {
                    let total = state.trade_count.load(Ordering::Relaxed);
                    let resp = MarketProtocol::SnapshotResponse { total_trades: total };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&resp)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                _ => Ok(vesicle),
            }
        }
    }, Some(MarketProtocol::CELL_GENOME.to_string())).await
}