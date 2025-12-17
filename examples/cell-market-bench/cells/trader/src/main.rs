use anyhow::Result;
use cell_sdk::cell_remote;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;

cell_remote!(Exchange = "exchange");

const CONCURRENCY: usize = 2;
const DURATION_SECS: u64 = 1;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("error").init();

    println!("--- TRADER ONLINE ---");

    // 1. Establish initial connection to cache the route
    let client = Exchange::Client::connect().await?;
    client.ping(0).await?;
    println!("Connected to Exchange via Router");

    let barrier = Arc::new(Barrier::new(CONCURRENCY));
    let mut handles = Vec::new();
    let start_signal = Instant::now();

    println!(
        "Starting Benchmark: {} Tasks, {} Seconds...",
        CONCURRENCY, DURATION_SECS
    );

    for i in 0..CONCURRENCY {
        let b = barrier.clone();
        // Each task gets its own client connection (simulating multiple users)
        // In this SDK, Client holds a Synapse which is thread-safe, so we could clone it too.
        let task_client = Exchange::Client::connect().await?;

        handles.push(tokio::spawn(async move {
            b.wait().await; // Wait for everyone to be ready

            let mut ops = 0u64;
            let start = Instant::now();
            let limit = Duration::from_secs(DURATION_SECS);

            while start.elapsed() < limit {
                // Alternate between ping and order to test different payload sizes
                if i % 2 == 0 {
                    let _ = task_client.ping(ops).await;
                } else {
                    let _ = task_client.place_order("BTC".to_string(), 100, 1).await;
                }
                ops += 1;
            }
            ops
        }));
    }

    let mut total_ops = 0;
    for h in handles {
        total_ops += h.await?;
    }

    let duration = start_signal.elapsed().as_secs_f64();
    let rps = total_ops as f64 / duration;

    println!("──────────────────────────────");
    println!("Benchmark Results");
    println!("──────────────────────────────");
    println!("Total Operations: {}", total_ops);
    println!("Duration:         {:.2}s", duration);
    println!("Throughput:       {:.2} msgs/sec", rps);
    println!(
        "Avg Latency:      {:.2} ms",
        (1000.0 / (rps / CONCURRENCY as f64))
    );
    println!("──────────────────────────────");

    Ok(())
}
