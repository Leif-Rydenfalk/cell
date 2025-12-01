use anyhow::Result;
use cell_sdk::cell_remote;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::env;

cell_remote!(ExchangeClient = "exchange");

// --- Protocol Definitions ---
#[derive(cell_sdk::serde::Serialize, cell_sdk::serde::Deserialize, cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize)]
#[serde(crate = "cell_sdk::serde")]
#[archive(check_bytes)]
#[archive(crate = "cell_sdk::rkyv")]
enum ExchangeServiceProtocol {
    PlaceOrder { symbol: String, amount: u64, side: u8 },
    SubmitBatch { count: u32 },
    IngestData { data: Vec<u8> },
    Ping { seq: u64 }, // <--- NEW: Honest Ping
}

#[derive(cell_sdk::serde::Serialize, cell_sdk::serde::Deserialize, cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize)]
#[serde(crate = "cell_sdk::serde")]
#[archive(check_bytes)]
#[archive(crate = "cell_sdk::rkyv")]
enum ExchangeServiceResponse {
    PlaceOrder(Result<u64, String>),
    SubmitBatch(Result<u64, String>),
    IngestData(Result<u64, String>),
    Ping(Result<u64, String>), // <--- NEW: Honest Pong
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

    // ✅ Honest Ping-Pong: Send seq, wait for echo, return.
    pub async fn ping(&mut self, seq: u64) -> Result<u64> {
        let req = ExchangeServiceProtocol::Ping { seq };
        let resp = self.connection().fire::<ExchangeServiceProtocol, ExchangeServiceResponse>(&req).await?;
        match resp.deserialize()? {
            ExchangeServiceResponse::Ping(Ok(res)) => Ok(res),
            ExchangeServiceResponse::Ping(Err(e)) => anyhow::bail!(e),
            _ => anyhow::bail!("Invalid response type"),
        }
    }
}

fn format_num(n: f64) -> String {
    let s = format!("{:.0}", n);
    let mut result = String::new();
    let mut count = 0;
    for c in s.chars().rev() {
        if count > 0 && count % 3 == 0 { result.push(','); }
        result.push(c);
        count += 1;
    }
    result.chars().rev().collect()
}

#[derive(Clone)]
enum Mode {
    Batch(u32),
    Bytes(usize),
    Ping, // <--- NEW
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    
    let (concurrency, mode) = if args.len() > 1 {
        let conc = args[1].parse().unwrap_or(1);
        let mode_str = args.get(2).map(|s| s.as_str()).unwrap_or("ping");
        
        let m = match mode_str {
            "bytes" => {
                 let size = args.get(3).unwrap_or(&String::from("100")).parse().unwrap_or(100);
                 Mode::Bytes(size)
            },
            "batch" => {
                 let size = args.get(3).unwrap_or(&String::from("100")).parse().unwrap_or(100);
                 Mode::Batch(size as u32)
            },
            _ => Mode::Ping,
        };
        (conc, m)
    } else {
        println!("Usage: trader <concurrency> <mode: ping|batch|bytes> <size>");
        (1, Mode::Ping)
    };

    println!("--------------------------------------------------");
    println!(" CELL BENCHMARK SUITE");
    println!("--------------------------------------------------");
    println!(" Concurrency : {} tasks", concurrency);
    match mode {
        Mode::Batch(s) => println!(" Mode        : Batch Orders (Size: {})", s),
        Mode::Bytes(s) => println!(" Mode        : Bandwidth (Payload: {} bytes)", s),
        Mode::Ping     => println!(" Mode        : Honest Ping-Pong (Latency Focus)"),
    }
    println!("--------------------------------------------------");

    let req_count = Arc::new(AtomicU64::new(0));
    let latency_sum_ns = Arc::new(AtomicU64::new(0));

    let r_req = req_count.clone();
    let r_lat = latency_sum_ns.clone();
    let r_mode = mode.clone();

    tokio::spawn(async move {
        let mut last_req = 0;
        let mut last_lat = 0;
        let mut start_time = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = Instant::now();
            let elapsed = now.duration_since(start_time).as_secs_f64();
            start_time = now;

            let curr_req = r_req.load(Ordering::Relaxed);
            let curr_lat = r_lat.load(Ordering::Relaxed);

            let delta_req = curr_req - last_req;
            let delta_lat = curr_lat - last_lat;

            let rps = delta_req as f64 / elapsed;
            
            // Raw RTT average
            let avg_ns = if delta_req > 0 { delta_lat as f64 / delta_req as f64 } else { 0.0 };
            
            let latency_str = if avg_ns < 1000.0 {
                format!("{:.0} ns", avg_ns)
            } else {
                format!("{:.2} µs", avg_ns / 1000.0)
            };

            match r_mode {
                Mode::Ping => {
                     println!(
                        "Ping RTT: {:>10} | QPS: {:>10}",
                        latency_str, format_num(rps)
                    );
                },
                Mode::Batch(batch_size) => {
                    let tps = rps * batch_size as f64;
                    println!(
                        "RPS: {:>9} | TPS: {:>11} | Latency: {:>7} | Batch: {}",
                        format_num(rps), format_num(tps), latency_str, batch_size
                    );
                },
                Mode::Bytes(_) => {
                    println!("RPS: {:>9} | Latency: {:>7}", format_num(rps), latency_str);
                }
            }

            last_req = curr_req;
            last_lat = curr_lat;
        }
    });

    let mut handles = vec![];

    for i in 0..concurrency {
        let t_req = req_count.clone();
        let t_lat = latency_sum_ns.clone();
        let task_mode = mode.clone();

        let handle = tokio::spawn(async move {
            let mut client = match ExchangeClient::connect().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Task {} failed: {}", i, e);
                    return;
                }
            };
            
            let mut seq = 0u64;
            
            let payload = if let Mode::Bytes(size) = task_mode {
                Some(vec![0u8; size])
            } else {
                None
            };

            loop {
                let start = Instant::now();
                
                let res = match task_mode {
                    Mode::Ping => {
                        seq = seq.wrapping_add(1);
                        client.ping(seq).await
                    },
                    Mode::Batch(batch_size) => {
                        client.submit_batch(batch_size).await
                    },
                    Mode::Bytes(_) => {
                        client.ingest_data(payload.clone().unwrap()).await
                    }
                };

                let duration = start.elapsed().as_nanos() as u64;

                match res {
                    Ok(_) => {
                        t_req.fetch_add(1, Ordering::Relaxed);
                        t_lat.fetch_add(duration, Ordering::Relaxed);
                    }
                    Err(_) => break, 
                }
            }
        });
        handles.push(handle);
    }

    for h in handles {
        let _ = h.await;
    }

    Ok(())
}