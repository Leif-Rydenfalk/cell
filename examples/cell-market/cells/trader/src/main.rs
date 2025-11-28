use anyhow::Result;
use cell_sdk::Synapse;
use std::time::Duration;
use protocol::MarketMsg;

#[tokio::main]
async fn main() -> Result<()> {
    let mut conn = loop {
        match Synapse::grow("exchange").await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    };

    println!("[Trader] Connected. Starting Batch Trading.");

    // Batch size of 100
    let batch_size = 100;
    
    loop {
        // Instead of firing 1 msg, we fire a batch representation
        let order = MarketMsg::SubmitBatch { count: batch_size };

        match conn.fire(order).await {
            Ok(_) => {
                // Yield occasionally to be a good citizen
                // tokio::task::yield_now().await;
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                break;
            }
        }
    }
    
    Ok(())
}