use anyhow::Result;
use cell_sdk::cell_remote;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error};

// Generates ExchangeClient (alias for ExchangeServiceClient) and imports DNA
cell_remote!(ExchangeClient = "exchange");

// Helper to unwrap the double Result logic
fn unwrap_response<T>(res: anyhow::Result<Result<T, String>>) -> Result<T> {
    match res? {
        Ok(val) => Ok(val),
        Err(e) => anyhow::bail!("Service Error: {}", e),
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
    Ping,
}

// --- Sharded Counters ---
const SHARDS: usize = 64; 

struct ShardedCounter {
    shards: [AtomicU64; SHARDS],
}

impl ShardedCounter {
    fn new() -> Self {
        const NEW_SHARD: AtomicU64 = AtomicU64::new(0);
        Self {
            shards: [NEW_SHARD; SHARDS],
        }
    }

    #[inline]
    fn inc(&self, n: u64) {
        let idx = shard_idx();
        self.shards[idx].fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    fn load(&self) -> u64 {
        self.shards.iter().map(|s| s.load(Ordering::Relaxed)).sum()
    }
}

fn shard_idx() -> usize {
    std::thread_local! {
        static IDX: usize = {
            let mut hasher = DefaultHasher::new();
            std::thread::current().id().hash(&mut hasher);
            (hasher.finish() as usize) & (SHARDS - 1)
        };
    }
    IDX.with(|i| *i)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

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
        Mode::Ping     => println!(" Mode        : Honest Ping-Pong (Integrity Verified)"),
    }
    println!("--------------------------------------------------");

    let req_count = Arc::new(ShardedCounter::new());
    let latency_sum_ns = Arc::new(ShardedCounter::new());

    let r_req = req_count.clone();
    let r_lat = latency_sum_ns.clone();
    let r_mode = mode.clone();

    // Reporter Task
    tokio::spawn(async move {
        let mut last_req = 0;
        let mut last_lat = 0;
        let mut start_time = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = Instant::now();
            let elapsed = now.duration_since(start_time).as_secs_f64();
            start_time = now;

            let curr_req = r_req.load();
            let curr_lat = r_lat.load();

            let delta_req = curr_req - last_req;
            let delta_lat = curr_lat.saturating_sub(last_lat);

            let rps = delta_req as f64 / elapsed;
            let avg_ns = if delta_req > 0 { delta_lat as f64 / delta_req as f64 } else { 0.0 };
            
            let latency_str = if avg_ns < 1000.0 {
                format!("{:.0} ns", avg_ns)
            } else {
                format!("{:.2} µs", avg_ns / 1000.0)
            };

            match r_mode {
                Mode::Ping => {
                     println!(
                        "Ping RTT: {:>10} | QPS: {:>10} | Verified",
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
                    error!("Task {} failed: {}", i, e);
                    return;
                }
            };
            
            let mut seq = 0u64;
            let payload = if let Mode::Bytes(size) = task_mode { Some(vec![0u8; size]) } else { None };

            loop {
                let start = Instant::now();
                
                let res = match task_mode {
                    Mode::Ping => {
                        seq = seq.wrapping_add(1);
                        // Call the generated method directly
                        let val = unwrap_response(client.ping(seq).await);
                        
                        // INTEGRITY CHECK
                        if let Ok(v) = val {
                            if v != seq {
                                panic!("FATAL: Data corruption! Sent {} but got {}", seq, v);
                            }
                            Ok(0)
                        } else {
                            val.map(|_| 0)
                        }
                    },
                    Mode::Batch(batch_size) => {
                        unwrap_response(client.submit_batch(batch_size).await).map(|_| 0)
                    },
                    Mode::Bytes(_) => {
                        unwrap_response(client.ingest_data(payload.clone().unwrap()).await)
                    }
                };

                let duration = start.elapsed().as_nanos() as u64;

                match res {
                    Ok(_) => {
                        t_req.inc(1);
                        t_lat.inc(duration);
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