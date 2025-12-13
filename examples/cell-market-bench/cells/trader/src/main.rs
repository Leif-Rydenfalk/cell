use anyhow::Result;
use cell_sdk::cell_remote;
use std::time::Instant;

// "Exchange" becomes the module name. "exchange" is the cell name to spawn.
cell_remote!(Exchange = "exchange");

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    tracing::info!("Trader active. Connecting to Exchange...");
    
    // 1. Connects to "exchange.sock"
    // 2. If missing, compiles ../exchange and runs it in background
    let mut client = Exchange::Client::connect().await?;
    
    tracing::info!("Connected. Running ping test...");
    
    let start = Instant::now();
    let result = client.ping(100).await?;
    let duration = start.elapsed();
    
    assert_eq!(result, 100);
    tracing::info!("Success! RTT: {:?}", duration);
    
    Ok(())
}