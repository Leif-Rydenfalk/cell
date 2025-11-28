use anyhow::Result;
use cell_sdk::{Membrane, Synapse};
use cell_consensus::{RaftNode, RaftConfig, StateMachine};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use protocol::MarketMsg;

struct MarketState {
    trade_count: AtomicU64,
}

impl StateMachine for MarketState {
    fn apply(&self, command: &[u8]) {
        // If command is 4 bytes, treat it as a batch count
        if command.len() == 4 {
             let count = u32::from_le_bytes(command.try_into().unwrap());
             self.trade_count.fetch_add(count as u64, Ordering::Relaxed);
        } else {
             self.trade_count.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn snapshot(&self) -> Vec<u8> { vec![] }
    fn restore(&self, _snapshot: &[u8]) {}
}

#[tokio::main]
async fn main() -> Result<()> {
    let state = Arc::new(MarketState { trade_count: AtomicU64::new(0) });

    let wal_path = std::path::PathBuf::from("/tmp/market.wal");
    let config = RaftConfig { id: 1, storage_path: wal_path };
    let raft = RaftNode::new(config, state.clone()).await?;

    println!("[Exchange] Consensus Active. Spawning Traders...");

    for _ in 0..5 {
        tokio::spawn(async move {
            if let Err(e) = Synapse::grow("trader").await {
                if !e.to_string().contains("failed to bind socket") {
                    eprintln!("[Exchange] Error spawning trader: {}", e);
                }
            }
        });
    }

    println!("[Exchange] Listening for orders...");

    Membrane::bind("exchange", move |vesicle| {
        let raft = raft.clone();
        let state = state.clone();

        async move {
            let msg = cell_sdk::rkyv::from_bytes::<MarketMsg>(vesicle.as_slice())
                .map_err(|e| anyhow::anyhow!("Msg Error: {:?}", e))?;

            match msg {
                MarketMsg::PlaceOrder { .. } => {
                    let _ = raft.propose(vec![1]).await.unwrap();
                    let ack = MarketMsg::OrderAck { id: 1 };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&ack)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                MarketMsg::SubmitBatch { count } => {
                    // Optimized Batch Path
                    // We serialize the COUNT into the log, effectively compressing 100 entries into 4 bytes on disk
                    let cmd = count.to_le_bytes().to_vec();
                    let _ = raft.propose(cmd).await.unwrap();

                    let ack = MarketMsg::OrderAck { id: count as u64 };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&ack)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                MarketMsg::SnapshotRequest => {
                    let count = state.trade_count.load(Ordering::Relaxed);
                    let resp = MarketMsg::SnapshotResponse { total_trades: count };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&resp)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                _ => Ok(vesicle)
            }
        }
    }).await
}