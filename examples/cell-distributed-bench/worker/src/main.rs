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
    // We pass the schema JSON so the SDK can serve it to peers for compilation
    run_service_with_schema("worker", __CELL_SCHEMA__, |request_bytes| {
        // 1. ZERO-COPY VALIDATION
        // We verify the bytes are a valid request without allocating a new struct
        let req = cell_sdk::rkyv::check_archived_root::<WorkerBenchmarkRequest>(request_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid rkyv data: {}", e))?;

        // 2. EXECUTE WORK
        let start = Instant::now();

        // 'req' is a reference to the raw bytes. Accessing fields is instant.
        // Note: req.test_type is an ArchivedString, so we use .as_str()
        match req.test_type.as_str() {
            "cpu_intensive" => {
                let mut rng = rand::thread_rng();
                for _ in 0..req.iterations {
                    let mut sum = 0.0;
                    // Simulate matrix math or physics calc
                    for j in 0..100 {
                        sum += (j as f64 * rng.gen::<f64>()).sqrt();
                    }
                    // Prevent compiler optimization
                    if sum > 9e18 {
                        println!("{}", sum);
                    }
                }
            }
            "bandwidth" | "ping" => {
                // For bandwidth tests, the cost is just receiving the payload (already done)
            }
            _ => {}
        }

        let duration = start.elapsed();
        let throughput = if duration.as_secs_f64() > 0.0 {
            req.iterations as f64 / duration.as_secs_f64()
        } else {
            0.0
        };

        if req.iterations > 1000 {
            println!(
                "🔧 Worker {} finished {} ops in {:?}",
                req.worker_id, req.iterations, duration
            );
        }

        // 3. SERIALIZE RESPONSE
        let response = WorkerBenchmarkResponse {
            iterations_completed: req.iterations,
            duration_ms: duration.as_millis() as u64,
            throughput,
        };

        let bytes = cell_sdk::rkyv::to_bytes::<_, 256>(&response)?;
        Ok(bytes.into_vec())
    })
}
