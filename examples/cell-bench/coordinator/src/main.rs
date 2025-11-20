use anyhow::Result;
use cell_sdk::*;
use std::time::{Duration, Instant};

fn main() -> Result<()> {
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

    // --- CONFIGURATION ---
    let iterations = 5_000;
    let payload_size = 4096; // 4KB Payload
                             // ---------------------

    println!("Coordinator starting benchmark.");
    println!("TARGET: {} requests", iterations);
    println!("PAYLOAD: {} bytes", payload_size);

    // Pre-allocate payload
    let payload_data = vec![7u8; payload_size];
    let mut latencies = Vec::with_capacity(iterations);

    let total_start = Instant::now();

    for i in 0..iterations {
        let job = WorkLoad {
            id: i as u32,
            iterations: 100,
            payload: payload_data.clone(),
        };

        let req_start = Instant::now();

        // RPC Call
        let _res: WorkResult = call_as!(worker, job)?;

        latencies.push(req_start.elapsed());

        if i % 1000 == 0 {
            println!(".. {} / {}", i, iterations);
        }
    }

    let total_duration = total_start.elapsed();

    // --- STATISTICS ---
    let total_ops = iterations as f64;
    let total_seconds = total_duration.as_secs_f64();

    let rps = total_ops / total_seconds;

    // Calculate Bandwidth (Outbound + Inbound approx)
    // 4KB Out + tiny response. We'll count just payload for throughput.
    let total_mb = (total_ops * payload_size as f64) / 1_000_000.0;
    let mbps = total_mb / total_seconds;

    let avg_latency = latencies.iter().sum::<Duration>() / iterations as u32;
    let max_latency = latencies.iter().max().unwrap();
    let min_latency = latencies.iter().min().unwrap();

    println!("\n============== RESULTS ==============");
    println!("Time Taken:   {:.2?}", total_duration);
    println!("Requests:     {}", iterations);
    println!("Payload:      4KB");
    println!("-------------------------------------");
    println!("Throughput:   {:.2} Req/sec", rps);
    println!("Bandwidth:    {:.2} MB/s (Payload only)", mbps);
    println!("-------------------------------------");
    println!("Avg Latency:  {:.2?}", avg_latency);
    println!("Min Latency:  {:.2?}", min_latency);
    println!("Max Latency:  {:.2?}", max_latency);
    println!("=====================================");

    Ok(())
}
