use anyhow::Result;
use cell_sdk::{Membrane, Synapse};
use cell_consensus::{RaftNode, RaftConfig, StateMachine};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use protocol::MarketMsg;

struct MarketState {
    trade_count: AtomicU64,
}

impl StateMachine for MarketState {
    fn apply(&self, _command: &[u8]) {
        self.trade_count.fetch_add(1, Ordering::Relaxed);
    }
    fn snapshot(&self) -> Vec<u8> { vec![] }
    fn restore(&self, _snapshot: &[u8]) {}
}

#[tokio::main]
async fn main() -> Result<()> {
    let state = Arc::new(MarketState { trade_count: AtomicU64::new(0) });

    let wal_path = std::path::PathBuf::from("/tmp/market.wal");
    let config = RaftConfig {
        id: 1,
        storage_path: wal_path,
    };
    let raft = RaftNode::new(config, state.clone()).await?;

    println!("[Exchange] Consensus Active. Spawning Traders...");

    for _ in 0..5 {
        tokio::spawn(async move {
            // FIX: Handle timeout for client-only cells
            match Synapse::grow("trader").await {
                Ok(_) => { /* Trader has a membrane (future proofing) */ },
                Err(e) => {
                    // If the error is a timeout waiting for socket, it just means
                    // the trader is up but didn't bind a server socket.
                    if e.to_string().contains("failed to bind socket") {
                        // This is expected for pure worker drones
                        println!("[Exchange] Trader drone launched (Client Mode)");
                    } else {
                        eprintln!("[Exchange] Failed to spawn trader: {}", e);
                    }
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
                MarketMsg::PlaceOrder { symbol: _, amount: _, side: _ } => {
                    let _ = raft.propose(vec![1]).await.unwrap();

                    let ack = MarketMsg::OrderAck { id: 1 };
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