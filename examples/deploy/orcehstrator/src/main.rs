// deploy/orchestrator/src/main.rs
use cell_sdk::{cell_remote, tissue::Tissue};
use std::time::Instant;

cell_remote!(Worker = "worker");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .init();
    
    println!("🎯 Orchestrator starting...");
    println!("📡 Discovering workers...");
    
    // Connect to worker swarm
    let mut swarm = Tissue::connect("worker").await?;
    
    println!("✅ Connected to worker swarm\n");
    
    // Run benchmark
    println!("🚀 Running 10,000 task benchmark...");
    println!("────────────────────────────────────────");
    
    let start = Instant::now();
    let num_tasks = 10_000;
    
    let mut completed = 0;
    let mut total_duration_ms = 0u64;
    
    for i in 0..num_tasks {
        let task = Worker::Task {
            id: format!("task-{}", i),
            payload: format!("payload-{}", i).into_bytes(),
        };
        
        // Distribute task to any available worker
        let result = swarm.distribute(&Worker::WorkerServiceProtocol::Process { task }).await?;
        let result_enum = result.deserialize()?;
        
        if let Worker::WorkerServiceResponse::Process(r) = result_enum {
            completed += 1;
            total_duration_ms += r.duration_ms;
            
            if completed % 1000 == 0 {
                println!("✓ {} tasks completed...", completed);
            }
        }
    }
    
    let total_elapsed = start.elapsed();
    
    println!("────────────────────────────────────────");
    println!("✅ Benchmark Complete!\n");
    println!("📊 Results:");
    println!("  Total Tasks:     {}", num_tasks);
    println!("  Completed:       {}", completed);
    println!("  Total Time:      {:.2}s", total_elapsed.as_secs_f64());
    println!("  Throughput:      {:.0} tasks/sec", 
             num_tasks as f64 / total_elapsed.as_secs_f64());
    println!("  Avg Task Time:   {:.2}ms", total_duration_ms as f64 / completed as f64);
    
    // Get worker statistics
    println!("\n📈 Worker Statistics:");
    println!("────────────────────────────────────────");
    
    let stats_results = swarm.broadcast(&Worker::WorkerServiceProtocol::Stats).await;
    
    for (i, result) in stats_results.iter().enumerate() {
        if let Ok(resp) = result {
            if let Ok(stats_enum) = resp.deserialize() {
                if let Worker::WorkerServiceResponse::Stats(stats) = stats_enum {
                    println!("Worker {}: {} tasks | {}s uptime | {:.1}% CPU",
                             i + 1,
                             stats.tasks_completed,
                             stats.uptime_secs,
                             stats.cpu_percent
                    );
                }
            }
        }
    }
    
    Ok(())
}