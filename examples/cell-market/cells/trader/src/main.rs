use anyhow::Result;
use cell_sdk::{Synapse, protein};
use std::time::Duration;

// --- SCHEMA DEFINITION (Client) ---
// This reads ~/.cell/schema/MarketV1.lock
// If I change a field here, the compiler will panic!
#[protein(class = "MarketV1")]
pub enum MarketMsg {
    PlaceOrder {
        symbol: String,
        amount: u64,
        side: u8,
    },
    SubmitBatch {
        count: u32,
    },
    OrderAck {
        id: u64,
    },
    SnapshotRequest,
    SnapshotResponse {
        total_trades: u64,
    },
}
// ----------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let mut conn = loop {
        match Synapse::grow("exchange").await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    };

    println!("[Trader] Connected (FP: {:x}).", MarketMsg::SCHEMA_FINGERPRINT);

    loop {
        let order = MarketMsg::SubmitBatch { count: 100 };
        match conn.fire(order).await {
            Ok(_) => {}
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                break;
            }
        }
    }
    
    Ok(())
}