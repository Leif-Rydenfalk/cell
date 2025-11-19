use anyhow::Result;
use cell_sdk::*;
use rand::Rng;
use std::time::Instant;

service_schema! {
    service: worker,
    request: WorkerBenchmarkRequest {
        worker_id: u32,
        iterations: u32,
        payload_size: usize,
        test_type: String,
        data_blob: Vec<u8>,
    },
    response: WorkerBenchmarkResponse {
        iterations_completed: u32,
        duration_ms: u64,
        throughput: f64,
    }
}

fn main() -> Result<()> {
    run_service_with_schema("worker", __CELL_SCHEMA__, |request_bytes| {
        // ZERO-COPY: We validate the buffer in place.
        // We do NOT allocate a native WorkerBenchmarkRequest struct.
        // 'req' becomes a reference into 'request_bytes'.
        let req = cell_sdk::rkyv::check_archived_root::<WorkerBenchmarkRequest>(request_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid rkyv data: {}", e))?;

        // req is &ArchivedWorkerBenchmarkRequest

        if req.iterations <= 1000 {
            // req.data_blob behaves like a slice (ArchivedVec)
            println!(
                "🔧 Worker {} received {} bytes",
                req.worker_id,
                req.data_blob.len()
            );
        }

        let start = Instant::now();
        let mut rng = rand::thread_rng();

        // ArchivedString requires .as_str() to compare with literal
        match req.test_type.as_str() {
            "bandwidth" => {}
            "ping" => {}
            "cpu_intensive" => {
                for _ in 0..req.iterations {
                    let mut sum = 0.0;
                    for j in 0..1000 {
                        sum += (j as f64 * rng.gen::<f64>()).sqrt();
                    }
                    if sum > 9e18 {
                        println!("{}", sum);
                    }
                }
            }
            _ => {}
        }

        let duration = start.elapsed();
        let throughput = if duration.as_secs_f64() > 0.0 {
            req.iterations as f64 / duration.as_secs_f64()
        } else {
            0.0
        };

        // Construct response (Native struct)
        let response = WorkerBenchmarkResponse {
            iterations_completed: req.iterations,
            duration_ms: duration.as_millis() as u64,
            throughput,
        };

        // Serialize response using rkyv
        let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&response)?;
        Ok(bytes.into_vec())
    })
}
