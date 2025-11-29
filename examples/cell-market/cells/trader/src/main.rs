use anyhow::Result;
use cell_sdk::cell_remote;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Auto-generate client from source
cell_remote!(ExchangeClient = "exchange");

#[tokio::main]
async fn main() -> Result<()> {
    println!("[Trader] Benchmark Tool v1.0");

    let mut exchange = ExchangeClient::connect().await?;
    println!("[Trader] Connected to {}", exchange.address());

    // Metrics
    let req_count = Arc::new(AtomicU64::new(0));
    let report_counter = req_count.clone();

    // Spawn Reporting Task
    tokio::spawn(async move {
        let mut last_count = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let current = report_counter.load(Ordering::Relaxed);
            let delta = current - last_count;
            let orders_per_sec = delta * 100; // Batch size is 100
            
            println!(
                "--> {:>8} Req/s | {:>10} Orders/s | Total: {}", 
                delta, 
                orders_per_sec,
                current * 100
            );
            
            last_count = current;
        }
    });

    println!("[Trader] Starting flood (Batch Size: 100)...");
    
    // Tight Loop
    loop {
        // We verify the call succeeds, but don't parse the response to measure
        // raw throughput of the Cell RPC system.
        match exchange.submit_batch(100).await {
            Ok(_) => {
                req_count.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                eprintln!("[Trader] Error: {}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}