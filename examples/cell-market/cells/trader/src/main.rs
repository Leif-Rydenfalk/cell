use anyhow::Result;
use cell_sdk::Synapse;
use std::time::Duration;
use protocol::MarketMsg;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Connection Logic
    // We attempt to connect to the 'exchange'. 
    // If it's not up, Synapse::grow will try to spawn it (idempotent),
    // or we just loop until the socket is ready.
    let mut conn = loop {
        match Synapse::grow("exchange").await {
            Ok(c) => break c,
            Err(e) => {
                eprintln!("[Trader] Waiting for Exchange... ({})", e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    };

    println!("[Trader] Connected to Exchange. Starting High-Frequency Trading.");

    // 2. Trading Loop
    loop {
        let order = MarketMsg::PlaceOrder {
            symbol: "CELL".to_string(),
            amount: 100,
            side: 0 // Buy
        };

        match conn.fire(order).await {
            Ok(_) => {
                // Success. 
                // We yield to prevent this tight loop from starving the runtime 
                // if we are running single-threaded, though in this architecture
                // we are IO bound mostly.
                tokio::task::yield_now().await;
            }
            Err(e) => {
                eprintln!("[Trader] Connection lost: {}. Reconnecting...", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
                // Re-connect logic would go here in a robust app
                break; 
            }
        }
    }
    
    Ok(())
}