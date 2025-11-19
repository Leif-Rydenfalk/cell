use anyhow::Result;
use cell_sdk::rkyv::Deserialize;
use cell_sdk::*;
use std::thread;
use std::time::Instant;

service_schema! {
    service: coordinator,
    request: BenchmarkRequest {
        test_type:     String,
        iterations:    u32,
        payload_size:  usize,
        worker_count:  u32,
    },
    response: BenchmarkResponse {
        total_duration_ms: u64,
        throughput:        f64,
        summary:           String,
    }
}

#[derive(
    cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub struct WorkerBenchmarkRequest {
    pub worker_id: u32,
    pub iterations: u32,
    pub payload_size: usize,
    pub test_type: String,
    pub data_blob: Vec<u8>,
}

#[derive(
    cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub struct WorkerResult {
    pub worker_id: u32,
    pub iterations_completed: u32,
    pub duration_ms: u64,
    pub throughput: f64,
}

#[derive(cell_sdk::rkyv::Archive, cell_sdk::rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub struct WorkerBenchmarkResponse {
    pub iterations_completed: u32,
    pub duration_ms: u64,
    pub throughput: f64,
}

// We also need Aggregate structs if we want to use the aggregator,
// but for this direct test we can calculate locally or use these stubs.
#[derive(
    cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub struct AggregateRequest {
    pub worker_results: Vec<WorkerResult>,
    pub test_type: String,
}

#[derive(cell_sdk::rkyv::Archive, cell_sdk::rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub struct AggregateResponse {
    pub total_throughput: f64,
    pub avg_latency_ms: f64,
    pub summary: String,
}

fn main() -> Result<()> {
    run_service_with_schema("coordinator", __CELL_SCHEMA__, |request_bytes| {
        // 1. TRY JSON (For CLI usage)
        match serde_json::from_slice::<BenchmarkRequest>(request_bytes) {
            Ok(req) => return run_benchmark(req),
            Err(e) => {
                if request_bytes.first() == Some(&b'{') {
                    let json_str = String::from_utf8_lossy(request_bytes);
                    println!("❌ JSON Parse Error: {}", e);
                    println!("   Input: {}", json_str);
                    return Err(anyhow::anyhow!("JSON Error: {}", e));
                }
            }
        }

        // 2. TRY RKYV
        let archived = cell_sdk::rkyv::check_archived_root::<BenchmarkRequest>(request_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid data: {}", e))?;

        let req: BenchmarkRequest = archived.deserialize(&mut cell_sdk::rkyv::Infallible)?;
        run_benchmark(req)
    })
}

fn run_benchmark(req: BenchmarkRequest) -> Result<Vec<u8>> {
    println!("🎯 STARTING BENCHMARK: {} workers", req.worker_count);
    let start = Instant::now();

    let blob_data = vec![1u8; req.payload_size];
    let mut handles = vec![];

    // Distribute work
    let it_per_thread = req.iterations / req.worker_count;

    for id in 0..req.worker_count {
        let t_it = it_per_thread;
        let t_type = req.test_type.clone();
        let t_blob = blob_data.clone();
        let payload_size = req.payload_size;

        handles.push(thread::spawn(move || -> Result<WorkerResult> {
            let mut client = CellClient::connect("worker").expect("Failed to connect to worker");

            let mut total_ops = 0;

            if t_type == "ping" {
                // PING MODE: Loop here (Network Bound)
                let worker_req = WorkerBenchmarkRequest {
                    worker_id: id,
                    iterations: 1,
                    payload_size,
                    test_type: t_type.clone(),
                    data_blob: t_blob,
                };
                let payload = cell_sdk::rkyv::to_bytes::<_, 1024>(&worker_req)?;

                for _ in 0..t_it {
                    let _ = client.call(&payload)?;
                    total_ops += 1;
                }
            } else {
                // COMPUTE MODE: Send once (Compute Bound)
                let worker_req = WorkerBenchmarkRequest {
                    worker_id: id,
                    iterations: t_it,
                    payload_size,
                    test_type: t_type.clone(),
                    data_blob: t_blob,
                };
                let payload = cell_sdk::rkyv::to_bytes::<_, 1024>(&worker_req)?;
                let _ = client.call(&payload)?;
                total_ops = t_it;
            }

            Ok(WorkerResult {
                worker_id: id,
                iterations_completed: total_ops,
                duration_ms: 0,
                throughput: 0.0,
            })
        }));
    }

    let mut total_ops = 0;
    for h in handles {
        if let Ok(Ok(res)) = h.join() {
            total_ops += res.iterations_completed;
        }
    }

    let duration = start.elapsed();
    let seconds = duration.as_secs_f64();

    let total_ops_f = total_ops as f64;
    let ops_per_sec = total_ops_f / seconds;

    // Calculate Bandwidth (Payload * Ops)
    let total_bytes = total_ops_f * req.payload_size as f64;
    let mb_per_sec = total_bytes / seconds / 1024.0 / 1024.0;
    let gb_per_sec = mb_per_sec / 1024.0;

    let summary = format!(
        "Benchmark '{}' Complete.\n   Type: {}\n   Ops: {}\n   Time: {:.4}s\n   Throughput: {:.2} ops/sec\n   Bandwidth: {:.2} MB/s ({:.4} GB/s)",
        req.test_type,
        if req.test_type == "ping" { "Network Bound (IPC)" } else { "Compute Bound (Worker)" },
        total_ops,
        seconds,
        ops_per_sec,
        mb_per_sec,
        gb_per_sec
    );

    println!("{}", summary);

    let response = BenchmarkResponse {
        total_duration_ms: duration.as_millis() as u64,
        throughput: ops_per_sec,
        summary,
    };

    Ok(serde_json::to_vec(&response)?)
}
