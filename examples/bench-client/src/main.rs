use cell_sdk::*;
use anyhow::Result;
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    println!("🔥 Cell Performance Benchmark\n");
    
    // Latency test
    println!("═══════════════════════════════════════");
    println!("Test 1: Latency (10,000 echo requests)");
    println!("═══════════════════════════════════════");
    
    let start = Instant::now();
    let iterations = 10_000;
    
    for i in 0..iterations {
        let response = call_as!(bench_echo, EchoRequest {
            data: format!("ping-{}", i),
        })?;
        
        if i == 0 {
            println!("First response: {:?}", response.data);
        }
    }
    
    let duration = start.elapsed();
    let avg_latency = duration / iterations;
    let throughput = iterations as f64 / duration.as_secs_f64();
    
    println!("\n📊 Results:");
    println!("   Total time:    {:?}", duration);
    println!("   Avg latency:   {:?}", avg_latency);
    println!("   Throughput:    {:.0} req/s", throughput);
    println!("   Per-call:      {:.2} µs", avg_latency.as_micros() as f64);
    
    // Throughput test with small payloads
    println!("\n═══════════════════════════════════════");
    println!("Test 2: Throughput (1,000 requests, 100 numbers each)");
    println!("═══════════════════════════════════════");
    
    let numbers: Vec<f64> = (0..100).map(|i| i as f64).collect();
    let start = Instant::now();
    let iterations = 1_000;
    
    for _ in 0..iterations {
        let _response = call_as!(bench_processor, ProcessRequest {
            numbers: numbers.clone(),
            operation: "sum".to_string(),
        })?;
    }
    
    let duration = start.elapsed();
    let throughput = iterations as f64 / duration.as_secs_f64();
    let data_throughput = (numbers.len() * iterations * 8) as f64 / duration.as_secs_f64() / 1_000_000.0;
    
    println!("\n📊 Results:");
    println!("   Total time:    {:?}", duration);
    println!("   Throughput:    {:.0} req/s", throughput);
    println!("   Data rate:     {:.2} MB/s", data_throughput);
    
    // Large payload test
    println!("\n═══════════════════════════════════════");
    println!("Test 3: Large Payloads (100 requests, 10,000 numbers each)");
    println!("═══════════════════════════════════════");
    
    let large_numbers: Vec<f64> = (0..10_000).map(|i| i as f64).collect();
    let payload_size = large_numbers.len() * 8; // bytes
    println!("   Payload size: {} KB", payload_size / 1024);
    
    let start = Instant::now();
    let iterations = 100;
    
    for _ in 0..iterations {
        let _response = call_as!(bench_processor, ProcessRequest {
            numbers: large_numbers.clone(),
            operation: "avg".to_string(),
        })?;
    }
    
    let duration = start.elapsed();
    let throughput = iterations as f64 / duration.as_secs_f64();
    let data_throughput = (payload_size * iterations) as f64 / duration.as_secs_f64() / 1_000_000.0;
    
    println!("\n📊 Results:");
    println!("   Total time:    {:?}", duration);
    println!("   Throughput:    {:.0} req/s", throughput);
    println!("   Data rate:     {:.2} MB/s", data_throughput);
    println!("   Avg latency:   {:.2} ms", duration.as_millis() as f64 / iterations as f64);
    
    // Very large payload test (tests length-prefixed framing)
    println!("\n═══════════════════════════════════════");
    println!("Test 4: Very Large Payloads (10 requests, 1M numbers each)");
    println!("═══════════════════════════════════════");
    
    let huge_numbers: Vec<f64> = (0..1_000_000).map(|i| (i % 1000) as f64).collect();
    let payload_size = huge_numbers.len() * 8; // bytes
    println!("   Payload size: {:.1} MB", payload_size as f64 / 1_000_000.0);
    
    let start = Instant::now();
    let iterations = 10;
    
    for i in 0..iterations {
        let response = call_as!(bench_processor, ProcessRequest {
            numbers: huge_numbers.clone(),
            operation: "sum".to_string(),
        })?;
        
        if i == 0 {
            println!("   First result: sum={}, count={}", response.result, response.count);
        }
    }
    
    let duration = start.elapsed();
    let throughput = iterations as f64 / duration.as_secs_f64();
    let data_throughput = (payload_size * iterations) as f64 / duration.as_secs_f64() / 1_000_000.0;
    
    println!("\n📊 Results:");
    println!("   Total time:    {:?}", duration);
    println!("   Throughput:    {:.1} req/s", throughput);
    println!("   Data rate:     {:.1} MB/s", data_throughput);
    println!("   Avg latency:   {:.0} ms", duration.as_millis() as f64 / iterations as f64);
    
    println!("\n✅ Benchmark complete!\n");
    
    Ok(())
}
