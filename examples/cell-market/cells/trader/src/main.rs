use anyhow::Result;
use cell_sdk::cell_remote;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::env;

cell_remote!(ExchangeClient = "exchange");

// --- Protocol Definitions (Shared Contract) ---
// In a full implementation, these would be in a shared crate or generated from schema.

#[derive(cell_sdk::serde::Serialize, cell_sdk::serde::Deserialize, cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize)]
#[serde(crate = "cell_sdk::serde")]
#[archive(check_bytes)]
#[archive(crate = "cell_sdk::rkyv")]
enum ExchangeServiceProtocol {
    PlaceOrder { symbol: String, amount: u64, side: u8 },
    SubmitBatch { count: u32 },
    IngestData { data: Vec<u8> },
}

#[derive(cell_sdk::serde::Serialize, cell_sdk::serde::Deserialize, cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize)]
#[serde(crate = "cell_sdk::serde")]
#[archive(check_bytes)]
#[archive(crate = "cell_sdk::rkyv")]
enum ExchangeServiceResponse {
    PlaceOrder(Result<u64, String>),
    SubmitBatch(Result<u64, String>),
    IngestData(Result<u64, String>),
}

// --- Client Implementation ---

impl ExchangeClient {
    pub async fn submit_batch(&mut self, count: u32) -> Result<u64> {
        let req = ExchangeServiceProtocol::SubmitBatch { count };
        let resp = self.connection().fire::<ExchangeServiceProtocol, ExchangeServiceResponse>(&req).await?;
        match resp.deserialize()? {
            ExchangeServiceResponse::SubmitBatch(Ok(res)) => Ok(res),
            ExchangeServiceResponse::SubmitBatch(Err(e)) => anyhow::bail!(e),
            _ => anyhow::bail!("Invalid response type"),
        }
    }

    pub async fn ingest_data(&mut self, data: Vec<u8>) -> Result<u64> {
        let req = ExchangeServiceProtocol::IngestData { data };
        let resp = self.connection().fire::<ExchangeServiceProtocol, ExchangeServiceResponse>(&req).await?;
        match resp.deserialize()? {
            ExchangeServiceResponse::IngestData(Ok(res)) => Ok(res),
            ExchangeServiceResponse::IngestData(Err(e)) => anyhow::bail!(e),
            _ => anyhow::bail!("Invalid response type"),
        }
    }
}

// --- Benchmark Runner ---

struct Config {
    concurrency: usize,
    mode: Mode,
}

#[derive(Clone)]
enum Mode {
    Batch(u32),     // Value is batch size (e.g., 100 orders per req)
    Bytes(usize),   // Value is payload size in bytes (e.g., 1024 bytes)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    
    // Default: 16 concurrent tasks, Batch mode with 100 items
    let (concurrency, mode) = if args.len() > 1 {
        let conc = args[1].parse().unwrap_or(16);
        let mode_str = args.get(2).map(|s| s.as_str()).unwrap_or("batch");
        let size = args.get(3).unwrap_or(&String::from("100")).parse().unwrap_or(100);
        
        let m = match mode_str {
            "bytes" => Mode::Bytes(size),
            _ => Mode::Batch(size as u32),
        };
        (conc, m)
    } else {
        println!("Usage: trader <concurrency> <mode: batch|bytes> <size>");
        println!("Defaults: 16 batch 100");
        (16, Mode::Batch(100))
    };

    println!("[BM] Starting Benchmark.");
    println!("[BM] Concurrency: {} tasks", concurrency);
    
    match &mode {
        Mode::Batch(s) => println!("[BM] Mode: Batch Orders (Size: {})", s),
        Mode::Bytes(s) => println!("[BM] Mode: Bandwidth (Payload: {} bytes)", s),
    }

    // Global Metrics
    let req_count = Arc::new(AtomicU64::new(0));
    let bytes_count = Arc::new(AtomicU64::new(0)); // Only used in bytes mode

    // 1. Spawn Reporter Task
    let r_req = req_count.clone();
    let r_bytes = bytes_count.clone();
    let r_mode_is_bytes = matches!(mode, Mode::Bytes(_));

    tokio::spawn(async move {
        let mut last_req = 0;
        let mut last_bytes = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let curr_req = r_req.load(Ordering::Relaxed);
            let curr_bytes = r_bytes.load(Ordering::Relaxed);

            let delta_req = curr_req - last_req;
            let delta_bytes = curr_bytes - last_bytes;

            if r_mode_is_bytes {
                let mbps = (delta_bytes as f64) / 1024.0 / 1024.0;
                println!("--> RPS: {:>6} | Throughput: {:>8.2} MB/s", delta_req, mbps);
            } else {
                // Assuming batch size is roughly constant for visualization, 
                // though strictly it's defined in the tasks.
                println!("--> RPS: {:>6} | Total Reqs: {}", delta_req, curr_req);
            }

            last_req = curr_req;
            last_bytes = curr_bytes;
        }
    });

    // 2. Spawn Load Generators
    let mut handles = vec![];

    for i in 0..concurrency {
        let t_req = req_count.clone();
        let t_bytes = bytes_count.clone();
        
        // Clone config data
        let task_mode = mode.clone();

        let handle = tokio::spawn(async move {
            // Establish a new connection per task to simulate distinct clients
            // or connection pooling.
            let mut client = match ExchangeClient::connect().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Task {} failed to connect: {}", i, e);
                    return;
                }
            };

            // Pre-allocate payload if in bytes mode
            let payload = if let Mode::Bytes(size) = task_mode {
                Some(vec![0u8; size])
            } else {
                None
            };

            loop {
                match task_mode {
                    Mode::Batch(batch_size) => {
                        match client.submit_batch(batch_size).await {
                            Ok(_) => {
                                t_req.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(_) => break, // Connection died
                        }
                    },
                    Mode::Bytes(_) => {
                        // clone payload cheap if wrapped, but here we clone vec. 
                        // In real high-perf, use Bytes/Arc.
                        // For this test, cloning overhead is part of the client cpu load.
                        match client.ingest_data(payload.clone().unwrap()).await {
                            Ok(n) => {
                                t_req.fetch_add(1, Ordering::Relaxed);
                                t_bytes.fetch_add(n, Ordering::Relaxed);
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
        handles.push(handle);
    }

    // Wait for all (they run forever usually)
    for h in handles {
        let _ = h.await;
    }

    Ok(())
}