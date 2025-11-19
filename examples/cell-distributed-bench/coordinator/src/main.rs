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
        avg_latency_ms:    f64,
        summary:           String,
    }
}

#[derive(
    serde::Serialize,
    serde::Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Debug,
    Clone,
)]
#[archive(check_bytes)]
pub struct WorkerResult {
    pub worker_id: u32,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct WorkerBenchmarkRequest {
    pub worker_id: u32,
    pub iterations: u32,
    pub payload_size: usize,
    pub test_type: String,
    pub data_blob: Vec<u8>,
}

fn main() -> Result<()> {
    run_service_with_schema("coordinator", __CELL_SCHEMA__, |request_bytes| {
        if let Ok(r) = serde_json::from_slice::<BenchmarkRequest>(request_bytes) {
            return run_benchmark(r);
        }
        let archived = cell_sdk::rkyv::check_archived_root::<BenchmarkRequest>(request_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid data: {}", e))?;
        let req: BenchmarkRequest = archived.deserialize(&mut cell_sdk::rkyv::Infallible)?;
        run_benchmark(req)
    })
}

fn run_benchmark(req: BenchmarkRequest) -> Result<Vec<u8>> {
    println!("🎯 DATA THROUGHPUT Benchmark starting (RKYV + BATCHED PIPELINING)...");
    println!("   Total Calls: {}", req.iterations);
    println!("   Payload/Call: {} bytes", req.payload_size);
    println!("   Threads: {}", req.worker_count);

    let blob_data = vec![1u8; req.payload_size];
    let blob_len = blob_data.len();
    let start = Instant::now();

    let it_per_thread = req.iterations / req.worker_count;
    let mut handles = vec![];

    for _ in 0..req.worker_count {
        let t_it = it_per_thread;
        let t_type = req.test_type.clone();
        let t_blob = blob_data.clone();

        handles.push(thread::spawn(move || {
            // BATCH SIZE: 64
            // We will buffer 64 requests, send them in one syscall, then read 64 responses.
            let batch_size = 64;
            let mut client = CellClient::connect_with_batch("../worker/run/cell.sock", batch_size)
                .expect("Failed to connect to worker");

            let worker_req = WorkerBenchmarkRequest {
                worker_id: 0,
                iterations: 1,
                payload_size: blob_len,
                test_type: t_type,
                data_blob: t_blob,
            };

            // Pre-serialize once to avoid CPU overhead in the loop (Pure IO test)
            let payload =
                cell_sdk::rkyv::to_bytes::<_, 1024>(&worker_req).expect("Failed to serialize");
            let payload_bytes = payload.as_slice();

            let mut sent_since_read = 0;

            for _ in 0..t_it {
                // Queue Request
                let flushed = client.queue_request(payload_bytes).expect("Queue failed");

                sent_since_read += 1;

                if flushed {
                    // If we just flushed (sent batch_size requests), we must read their responses
                    // to clear the socket buffer so the server doesn't block.
                    client.read_n_responses(batch_size).expect("Read failed");
                    sent_since_read = 0;
                }
            }

            // Cleanup: Flush remaining items
            client.flush_writes().expect("Final flush failed");
            if sent_since_read > 0 {
                client
                    .read_n_responses(sent_since_read)
                    .expect("Final read failed");
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let duration = start.elapsed();
    let seconds = duration.as_secs_f64();

    let total_calls = req.iterations as f64;
    let calls_per_sec = total_calls / seconds;
    let total_bytes = total_calls * req.payload_size as f64;
    let mb_per_sec = total_bytes / seconds / 1024.0 / 1024.0;
    let gb_per_sec = mb_per_sec / 1024.0;

    let summary = format!(
        "Results:\n   Time: {:?}\n   RPC Rate: {:.2} calls/sec\n   Bandwidth: {:.2} MB/s ({:.4} GB/s)",
        duration, calls_per_sec, mb_per_sec, gb_per_sec
    );
    println!("{}", summary);

    let out = BenchmarkResponse {
        total_duration_ms: duration.as_millis() as u64,
        throughput: calls_per_sec,
        avg_latency_ms: 0.0,
        summary,
    };

    Ok(serde_json::to_vec(&out)?)
}
