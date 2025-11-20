use anyhow::Result;
use cell_sdk::*;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

// We derive Clone so we can send the same payload structure multiple times
// without ownership issues inside the macro's internal closure.
#[derive(
    serde::Serialize, serde::Deserialize, cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, Clone,
)]
#[archive(crate = "cell_sdk::rkyv")]
#[archive(check_bytes)]
struct WorkLoad {
    id: u32,
    iterations: u32,
    payload: Vec<u8>,
}

#[derive(
    serde::Serialize,
    serde::Deserialize,
    cell_sdk::rkyv::Archive,
    cell_sdk::rkyv::Deserialize,
    Debug,
)]
#[archive(crate = "cell_sdk::rkyv")]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
struct WorkResult {
    processed: u32,
    checksum: u64,
}

fn main() -> Result<()> {
    println!("\n[Coordinator] Starting Benchmark Suite");
    println!("=======================================");

    // --- 1. Service Discovery ---
    // The worker might not be online immediately, or the Pheromone packet
    // might not have arrived at our Golgi router yet. We poll until success.
    println!("[System] Waiting for peer discovery...");

    let discovery_job = WorkLoad {
        id: 0,
        iterations: 1,
        payload: vec![],
    };

    let start_wait = Instant::now();
    loop {
        let ping_payload = discovery_job.clone();
        match call_as!(worker, ping_payload) {
            Ok(_) => {
                println!("\n[System] Peer 'worker' discovered and reachable.");
                break;
            }
            Err(_) => {
                if start_wait.elapsed().as_secs() > 15 {
                    panic!("[Error] Discovery timeout. Multicast might be disabled on network.");
                }
                std::thread::sleep(Duration::from_millis(500));

                use std::io::Write;
                print!(".");
                std::io::stdout().flush().ok();
            }
        }
    }

    // --- 2. Connection Pool Warmup ---
    // Establish the initial TCP connections before measuring timing.
    print!("[System] Warming up connection pools... ");
    run_test("Warmup", 1, 100, 1, false);
    println!("Done.\n");

    // --- 3. Latency Test ---
    // Single thread, minimal payload. Measures pure round-trip overhead.
    run_test("Latency Test (Single Thread)", 1, 10_000, 1, true);

    // --- 4. Throughput Test (RPS) ---
    // Multi-threaded (8), minimal payload. Tests concurrency and connection pooling.
    run_test("Max Throughput (8 Threads)", 1, 100_000, 8, true);

    // --- 5. Bandwidth Test ---
    // Multi-threaded (8), 64KB payload.
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

    // Barrier ensures all threads start sending requests at the exact same time
    let barrier = Arc::new(Barrier::new(threads + 1));
    let mut handles = Vec::new();

    let start_time = Instant::now();

    for _ in 0..threads {
        let b = barrier.clone();
        let size = payload_size;
        let iterations = iter_per_thread;

        handles.push(thread::spawn(move || {
            let payload = vec![7u8; size]; // Allocate payload once

            // Wait for global start signal
            b.wait();

            for i in 0..iterations {
                let job = WorkLoad {
                    id: i,
                    iterations: 5, // Minimal work on server side
                    payload: payload.clone(),
                };

                if let Err(e) = call_as!(worker, job) {
                    eprintln!("[Error] RPC failed: {}", e);
                    break;
                }
            }
        }));
    }

    // Release the threads
    barrier.wait();

    // Wait for completion
    for h in handles {
        h.join().unwrap();
    }

    let duration = start_time.elapsed();

    // --- Statistics Calculation ---
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
