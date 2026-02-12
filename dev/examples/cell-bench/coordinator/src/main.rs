use anyhow::Result;
use cell_sdk::*;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

// --- DATA STRUCTURES (PROTEINS) ---
// The #[protein] macro automatically derives:
// 1. Serde (for JSON/Polyglot compatibility)
// 2. Rkyv (for Zero-Copy Rust speed)
// 3. check_bytes (for Security)
// 4. Debug, Clone

#[protein]
struct WorkLoad {
    id: u32,
    iterations: u32,
    payload: Vec<u8>,
}

#[protein]
struct WorkResult {
    processed: u32,
    checksum: u64,
}

// ----------------------------------

fn main() -> Result<()> {
    println!("\n[Coordinator] Starting Benchmark Suite");
    println!("=======================================");

    // --- 1. Service Discovery ---
    println!("[System] Waiting for peer discovery...");

    let discovery_job = WorkLoad {
        id: 0,
        iterations: 1,
        payload: vec![],
    };

    let start_wait = Instant::now();
    loop {
        // We clone to keep the template alive for retries
        let ping_payload = discovery_job.clone();

        match call_as!(worker, ping_payload) {
            Ok(_) => {
                println!("\n[System] Peer 'worker' discovered and reachable.");
                break;
            }
            Err(_) => {
                if start_wait.elapsed().as_secs() > 15 {
                    panic!("[Error] Discovery timeout.");
                }
                std::thread::sleep(Duration::from_millis(500));
                use std::io::Write;
                print!(".");
                std::io::stdout().flush().ok();
            }
        }
    }

    // --- 2. Connection Pool Warmup ---
    print!("[System] Warming up connection pools... ");
    run_test("Warmup", 1, 100, 1, false);
    println!("Done.\n");

    // --- 3. Latency Test ---
    run_test("Latency Test (Single Thread)", 1, 10_000, 1, true);

    // --- 4. Throughput Test (RPS) ---
    run_test("Max Throughput (8 Threads)", 1, 100_000, 8, true);

    // --- 5. Bandwidth Test ---
    // Total Transfer: 64KB * 10,000 * 8 = ~5.2 GB
    run_test("Max Bandwidth (8 Threads, 64KB)", 65536, 10_000, 8, true);

    Ok(())
}

fn run_test(
    name: &str,
    payload_size: usize,
    iter_per_thread: u32,
    threads: usize,
    print_output: bool,
) {
    if print_output {
        println!("Running: {}", name);
    }

    let barrier = Arc::new(Barrier::new(threads + 1));
    let mut handles = Vec::new();
    let start_time = Instant::now();

    for _ in 0..threads {
        let b = barrier.clone();

        handles.push(thread::spawn(move || {
            let payload = vec![7u8; payload_size];
            b.wait(); // Sync start

            for i in 0..iter_per_thread {
                let job = WorkLoad {
                    id: i,
                    iterations: 5,
                    payload: payload.clone(),
                };

                if let Err(e) = call_as!(worker, job) {
                    eprintln!("[Error] RPC failed: {}", e);
                    break;
                }
            }
        }));
    }

    barrier.wait(); // Release threads
    for h in handles {
        h.join().unwrap();
    }

    let duration = start_time.elapsed();

    // Stats
    let total_requests = iter_per_thread as f64 * threads as f64;
    let total_bytes = total_requests * payload_size as f64;
    let rps = total_requests / duration.as_secs_f64();
    let mbps = (total_bytes / 1_000_000.0) / duration.as_secs_f64();

    if print_output {
        println!("   Duration:   {:.2?}", duration);
        println!("   Throughput: {:.0} Req/sec", rps);
        println!("   Bandwidth:  {:.2} MB/s", mbps);
        println!("---------------------------------------");
    }
}
