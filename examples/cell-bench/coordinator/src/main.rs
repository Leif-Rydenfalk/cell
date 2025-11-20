use anyhow::Result;
use cell_sdk::*;
use std::time::{Duration, Instant};

// --- DATA STRUCTURES ---
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

struct TestResult {
    name: String,
    payload_size: usize,
    rps: f64,
    mbps: f64,
    avg_latency: Duration,
}

fn main() -> Result<()> {
    println!("\n🧪 AUTOMATED CELL BENCHMARK SUITE 🧪");
    println!("======================================");

    // --- WARMUP ---
    // Wakes up the worker, ensures routes are hot.
    print!("Warming up...");
    run_scenario("Warmup", 1024, 100, false)?;
    println!(" Done.\n");

    let mut results = Vec::new();

    // --- SCENARIO 1: LATENCY & OVERHEAD ---
    // 1 Byte payload. Measures pure protocol overhead, handshake speed, and CPU efficiency.
    results.push(run_scenario("Max Throughput (Ping)", 1, 5_000, true)?);

    // --- SCENARIO 2: REAL WORLD ---
    // 4 KB payload. Typical size for JSON APIs or DB queries.
    results.push(run_scenario("Standard (4KB)", 4096, 5_000, true)?);

    // --- SCENARIO 3: MEDIUM DATA ---
    // 64 KB payload. Stresses buffer copying and fragmentation.
    results.push(run_scenario("Medium (64KB)", 65536, 2_000, true)?);

    // --- SCENARIO 4: BANDWIDTH ---
    // 1 MB payload. Stresses memory bandwidth and Encryption throughput.
    // 500 reqs * 1MB = 500MB transferred.
    results.push(run_scenario("Heavy (1MB)", 1_048_576, 500, true)?);

    // --- FINAL REPORT ---
    println!("\nBENCHMARK SUMMARY");
    println!(
        "{:<25} | {:<12} | {:<12} | {:<12}",
        "Scenario", "RPS", "Bandwidth", "Latency"
    );
    println!("{:-<25}-|-{:-<12}-|-{:-<12}-|-{:-<12}", "", "", "", "");

    let mut max_rps = 0.0;
    let mut max_bw = 0.0;

    for r in &results {
        if r.rps > max_rps {
            max_rps = r.rps;
        }
        if r.mbps > max_bw {
            max_bw = r.mbps;
        }

        println!(
            "{:<25} | {:<12} | {:<12} | {:.2?}",
            r.name,
            format!("{:.0} req/s", r.rps),
            format!("{:.2} MB/s", r.mbps),
            r.avg_latency
        );
    }

    println!("\nPEAK PERFORMANCE:");
    println!("   Max Throughput: {:.0} Requests/sec", max_rps);
    println!("   Max Bandwidth:  {:.2} MB/s", max_bw);
    println!("======================================\n");

    Ok(())
}

fn run_scenario(name: &str, size: usize, iter: u32, print: bool) -> Result<TestResult> {
    if print {
        println!("Running: {} ({} bytes x {} requests)", name, size, iter);
    }

    let payload_data = vec![7u8; size];
    let mut latencies = Vec::with_capacity(iter as usize);

    let total_start = Instant::now();

    for i in 0..iter {
        let job = WorkLoad {
            id: i,
            iterations: 10, // Minimal CPU work on worker side
            payload: payload_data.clone(),
        };

        let req_start = Instant::now();

        // RPC Call
        let _res: WorkResult = call_as!(worker, job)?;

        latencies.push(req_start.elapsed());

        // Progress bar
        if print && i % (iter / 10) == 0 && i > 0 {
            use std::io::Write;
            print!(".");
            std::io::stdout().flush().ok();
        }
    }
    if print {
        println!("");
    }

    let duration = total_start.elapsed();

    let rps = (iter as f64) / duration.as_secs_f64();
    let total_mb = (iter as f64 * size as f64) / 1_000_000.0;
    let mbps = total_mb / duration.as_secs_f64();
    let avg_lat = duration / iter;

    Ok(TestResult {
        name: name.to_string(),
        payload_size: size,
        rps,
        mbps,
        avg_latency: avg_lat,
    })
}
