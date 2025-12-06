use anyhow::Result;
use cell_sdk::cell_remote;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use rand::RngCore; // Needed to fill bytes

// --- THE NEW API ---
cell_remote!(Exchange = "exchange");

// --- UTILS ---

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

// --- SHARDED COUNTERS ---
const SHARDS: usize = 64; 
struct ShardedCounter { shards: [AtomicU64; SHARDS] }
impl ShardedCounter {
    fn new() -> Self { Self { shards: [const { AtomicU64::new(0) }; SHARDS] } }
    fn inc(&self, n: u64) {
        let idx = (std::thread::current().id().as_u64().get() as usize) & (SHARDS - 1);
        self.shards[idx].fetch_add(n, Ordering::Relaxed);
    }
    fn load(&self) -> u64 { self.shards.iter().map(|s| s.load(Ordering::Relaxed)).sum() }
}

trait ThreadIdExt { fn as_u64(&self) -> std::num::NonZeroU64; }
impl ThreadIdExt for std::thread::ThreadId {
    fn as_u64(&self) -> std::num::NonZeroU64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        unsafe { std::num::NonZeroU64::new_unchecked(hasher.finish() | 1) }
    }
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
            "bytes" => Mode::Bytes(args.get(3).unwrap_or(&"100".to_string()).parse().unwrap_or(100)),
            "batch" => Mode::Batch(args.get(3).unwrap_or(&"100".to_string()).parse().unwrap_or(100)),
            _ => Mode::Ping,
        };
        (conc, m)
    } else {
        println!("Usage: trader <concurrency> <mode: ping|batch|bytes> <size>");
        (1, Mode::Ping)
    };

    println!("--- CELL BENCHMARK ---");
    println!("Tasks: {}", concurrency);

    let req_count = Arc::new(ShardedCounter::new());
    let latency_sum = Arc::new(ShardedCounter::new());
    // NEW: Byte Counter
    let byte_count = Arc::new(ShardedCounter::new()); 

    let r_req = req_count.clone();
    let r_lat = latency_sum.clone();
    let r_bytes = byte_count.clone();
    let r_mode = mode.clone();

    // Reporter
    tokio::spawn(async move {
        let mut last_req = 0;
        let mut last_bytes = 0;
        let mut start = Instant::now();
        
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = Instant::now();
            let elapsed = now.duration_since(start).as_secs_f64();
            start = now;

            let curr_req = r_req.load();
            let curr_lat = r_lat.load(); 
            let curr_bytes = r_bytes.load();

            let delta_req = curr_req - last_req;
            let delta_bytes = curr_bytes - last_bytes;

            let rps = delta_req as f64 / elapsed;
            
            static mut LAST_LAT_SUM: u64 = 0;
            let delta_lat_sum = curr_lat - unsafe { LAST_LAT_SUM };
            unsafe { LAST_LAT_SUM = curr_lat };

            let avg_ns = if delta_req > 0 { delta_lat_sum as f64 / delta_req as f64 } else { 0.0 };
            let lat_str = if avg_ns < 1000.0 { format!("{:.0}ns", avg_ns) } else { format!("{:.2}µs", avg_ns/1000.0) };

            // Throughput Calc
            let throughput_bps = delta_bytes as f64 / elapsed;
            let throughput_str = if throughput_bps > 1_000_000_000.0 {
                format!("{:.2} GB/s", throughput_bps / 1_000_000_000.0)
            } else {
                format!("{:.2} MB/s", throughput_bps / 1_000_000.0)
            };

            match r_mode {
                Mode::Ping => println!("Ping RTT: {:>8} | QPS: {:>10}", lat_str, format_num(rps)),
                Mode::Batch(s) => println!("Batch({}) TPS: {:>10} | QPS: {:>9}", s, format_num(rps * s as f64), format_num(rps)),
                Mode::Bytes(s) => println!("Bytes({}) BW: {:>10} | QPS: {:>9} | Lat: {:>8}", s, throughput_str, format_num(rps), lat_str),
            }
            
            last_req = curr_req;
            last_bytes = curr_bytes;
        }
    });

    let mut handles = vec![];

    for i in 0..concurrency {
        let t_req = req_count.clone();
        let t_lat = latency_sum.clone();
        let t_bytes = byte_count.clone();
        let task_mode = mode.clone();

        handles.push(tokio::spawn(async move {
            let mut client = match Exchange::connect().await {
                Ok(c) => c,
                Err(e) => { error!("Task {}: Connect failed: {}", i, e); return; }
            };
            
            // Connection Verification Probe
            if let Ok(Ok(echo)) = client.ping(999).await {
                if echo == 999 {
                    if i == 0 { println!("Task 0: Connection verified with ping"); }
                } else {
                    error!("Task {}: Verification failed", i);
                    return;
                }
            }

            let mut seq = 0u64;
            
            // Pre-calculate payload for bytes mode to simulate high throughput data
            let (payload, expected_crc) = if let Mode::Bytes(s) = task_mode { 
                let mut p = vec![0u8; s];
                // Fill with random data so the CRC check is meaningful
                rand::thread_rng().fill_bytes(&mut p);
                let crc = crc32fast::hash(&p) as u64;
                (p, crc)
            } else { 
                (vec![], 0) 
            };
            
            let payload_size = payload.len() as u64;

            loop {
                let start = Instant::now();
                
                let res = match task_mode {
                    Mode::Ping => {
                        seq = seq.wrapping_add(1);
                        unwrap_response(client.ping(seq).await).map(|_| 0)
                    },
                    Mode::Batch(n) => unwrap_response(client.submit_batch(n).await).map(|_| 0),
                    Mode::Bytes(_) => {
                        // Integrity Check: The server returns the CRC of the data we sent.
                        // We compare it to our local calculation.
                        match unwrap_response(client.ingest_data(payload.clone()).await) {
                            Ok(server_crc) => {
                                if server_crc != expected_crc {
                                    panic!("DATA CORRUPTION DETECTED! Expected CRC: {}, Got: {}", expected_crc, server_crc);
                                }
                                Ok(server_crc)
                            },
                            Err(e) => Err(e),
                        }
                    },
                };

                let duration = start.elapsed().as_nanos() as u64;

                match res {
                    Ok(_) => {
                        t_req.inc(1);
                        t_lat.inc(duration);
                        if payload_size > 0 {
                            t_bytes.inc(payload_size);
                        }
                    }
                    Err(e) => {
                        error!("RPC Error: {}", e);
                        break;
                    }
                }
            }
        }));
    }

    for h in handles { let _ = h.await; }
    Ok(())
}