use anyhow::Result;
use cell_sdk::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

// --- PROTOCOL DEFINITION ---
// The signal_receptor! macro now automatically derives the #[protein] traits
// (Serde, Rkyv, CheckBytes) for the generated structs.
signal_receptor! {
    name: chatterbox,
    input: Gossip {
        from_pid: u32,
        sent_at_nanos: u128,
    },
    output: Ack {
        code: u8, // 1 = OK (Avoids String allocation for max speed)
    }
}

// --- STATS HOLDER ---
#[derive(Default)]
struct Stats {
    count: u64,
    total_latency_us: u128,
    window_start: Option<Instant>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let my_pid = std::process::id();

    // 1. HIGH SPEED CLIENT (Tight Loop)
    // Runs in the background to simulate mesh traffic
    tokio::spawn(async move {
        sleep(Duration::from_secs(2)).await; // Let system settle
        println!("Starting High-Frequency Client...");

        let mut req_count: u64 = 0;
        let mut total_rtt_us: u128 = 0;
        let mut min_rtt = u128::MAX;
        let mut max_rtt = 0;
        let mut last_report = Instant::now();

        loop {
            let start_time = Instant::now();
            
            // Use SystemTime for cross-process timestamping
            let now_system = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();

            let msg = Gossip {
                from_pid: my_pid,
                sent_at_nanos: now_system,
            };

            // Call As: Zero-copy serialization of the 'Gossip' struct
            match call_as!(chatterbox, msg) {
                Ok(_) => {
                    let rtt = start_time.elapsed().as_micros();
                    req_count += 1;
                    total_rtt_us += rtt;
                    if rtt < min_rtt { min_rtt = rtt; }
                    if rtt > max_rtt { max_rtt = rtt; }
                }
                Err(_) => {
                    // Tiny backoff on error to let the mesh recover
                    sleep(Duration::from_millis(5)).await;
                }
            }

            // Stats Reporting (Every 1000 requests to minimize syscall overhead)
            if req_count % 1000 == 0 {
                if last_report.elapsed() >= Duration::from_secs(1) {
                    let elapsed = last_report.elapsed().as_secs_f64();
                    let rps = req_count as f64 / elapsed;
                    let avg_rtt = if req_count > 0 { total_rtt_us / (req_count as u128) } else { 0 };

                    println!(
                        "CLIENT >> Tx: {} | RPS: {:.0} | RTT(us) Min:{}/Avg:{}/Max:{}",
                        req_count, rps, min_rtt, avg_rtt, max_rtt
                    );

                    // Reset
                    req_count = 0;
                    total_rtt_us = 0;
                    min_rtt = u128::MAX;
                    max_rtt = 0;
                    last_report = Instant::now();
                }
            }

            // Simple throttle to prevent 100% CPU usage in this demo
            sleep(Duration::from_millis(1)).await;
        }
    });

    // 2. SERVER (Receiver)
    println!("Chatterbox Node {} Listening.", my_pid);

    let server_stats = Arc::new(Mutex::new(Stats::default()));

    Membrane::bind(__GENOME__, move |vesicle| {
        let rx_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        // 1. Zero-Copy Validation
        let msg = cell_sdk::rkyv::check_archived_root::<Gossip>(vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Bad Data: {}", e))?;

        // 2. Access Data (No Allocation)
        let sent_at = msg.sent_at_nanos;

        // --- Stats Logic ---
        {
            let mut stats = server_stats.lock().unwrap();
            
            if stats.window_start.is_none() {
                stats.window_start = Some(Instant::now());
            }

            if rx_time > sent_at {
                let latency_ns = rx_time - sent_at;
                stats.total_latency_us += latency_ns / 1000;
            }
            stats.count += 1;

            // Log every 5 seconds
            if let Some(start) = stats.window_start {
                if start.elapsed() >= Duration::from_secs(5) {
                    let elapsed = start.elapsed().as_secs_f64();
                    let throughput = stats.count as f64 / elapsed;
                    let avg_lat = stats.total_latency_us / (stats.count as u128);

                    println!(
                        "SERVER >> Total: {} | Rate: {:.0}/s | Avg 1-Way: {} us",
                        stats.count, throughput, avg_lat
                    );

                    stats.count = 0;
                    stats.total_latency_us = 0;
                    stats.window_start = Some(Instant::now());
                }
            }
        }

        // 3. Response
        let resp = Ack { code: 1 };
        let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&resp)?.into_vec();
        Ok(vesicle::Vesicle::wrap(bytes))
    })
}