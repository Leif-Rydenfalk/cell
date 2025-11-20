use anyhow::Result;
use cell_sdk::*;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

#[derive(
    serde::Serialize, serde::Deserialize, cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize,
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
    println!("\nCELL BENCHMARK");
    println!("=======================================");

    // Warmup
    print!("Warming up connection pool...");
    run_test("Warmup", 1, 100, 1, false);
    println!(" Done.\n");

    // 1. Latency Test (Single Thread, small packet)
    run_test("Latency Test (1 Thread)", 1, 10_000, 1, true);

    // 2. High Throughput RPS (8 Threads, small packet)
    // This tests Connection Pooling + Concurrency
    run_test("Max RPS (8 Threads)", 1, 200_000, 8, true);

    // 3. Max Bandwidth (8 Threads, 64KB packet)
    // 64KB * 10,000 * 8 = ~5 GB data
    run_test("Bandwidth (8 Threads, 64KB)", 65536, 10_000, 8, true);

    Ok(())
}

fn run_test(name: &str, size: usize, iter_per_thread: u32, threads: usize, print: bool) {
    if print {
        println!("Running: {}", name);
    }

    let barrier = Arc::new(Barrier::new(threads + 1));
    let mut handles = Vec::new();

    let start = Instant::now();

    for t_id in 0..threads {
        let b = barrier.clone();
        let size = size;
        let iter = iter_per_thread;

        handles.push(thread::spawn(move || {
            let payload = vec![7u8; size];

            // Wait for start signal
            b.wait();

            for i in 0..iter {
                let job = WorkLoad {
                    id: i,
                    iterations: 5,
                    payload: payload.clone(),
                };
                let _res: WorkResult = call_as!(worker, job).unwrap();

                // Simple progress for thread 0
                if t_id == 0 && i % (iter / 5) == 0 && print {
                    use std::io::Write;
                    print!(".");
                    std::io::stdout().flush().ok();
                }
            }
        }));
    }

    // Sync start
    barrier.wait();

    // Wait join
    for h in handles {
        h.join().unwrap();
    }

    let duration = start.elapsed();
    if print {
        println!("");
    }

    let total_req = iter_per_thread as f64 * threads as f64;
    let total_bytes = total_req * size as f64;

    let rps = total_req / duration.as_secs_f64();
    let mbps = (total_bytes / 1_000_000.0) / duration.as_secs_f64();

    if print {
        println!("   Duration:   {:.2?}", duration);
        println!("   Throughput: {:.0} Req/sec", rps);
        println!("   Bandwidth:  {:.2} MB/s", mbps);
        println!("=======================================");
    }
}
