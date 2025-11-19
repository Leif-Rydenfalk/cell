use anyhow::Result;
use cell_sdk::rkyv::Deserialize;
use cell_sdk::*;
use std::thread;
use std::time::Instant;

// Define the schema for the Coordinator itself (so we can trigger it via CLI)
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

// We need to redefine the schemas of the services we talk to (Worker/Aggregator)
// In a real app, you might put these structs in a shared crate, but here we copy definition.
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

#[derive(
    cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize, Debug, Clone,
)]
#[archive(check_bytes)]
pub struct AggregateRequest {
    pub worker_results: Vec<WorkerResult>,
    pub test_type: String,
}

// Result types needed for deserialization
#[derive(cell_sdk::rkyv::Archive, cell_sdk::rkyv::Deserialize, Debug)]
#[archive(check_bytes)]
pub struct WorkerBenchmarkResponse {
    pub iterations_completed: u32,
    pub duration_ms: u64,
    pub throughput: f64,
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
        // 1. Parse Request
        let archived = cell_sdk::rkyv::check_archived_root::<BenchmarkRequest>(request_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid data: {}", e))?;
        // We deserialize fully here because we need to pass ownership to threads
        let req: BenchmarkRequest = archived.deserialize(&mut cell_sdk::rkyv::Infallible)?;

        run_benchmark(req)
    })
}

fn run_benchmark(req: BenchmarkRequest) -> Result<Vec<u8>> {
    println!(
        "🎯 STARTING BENCHMARK: {} threads, {} iter/thread",
        req.worker_count,
        req.iterations / req.worker_count
    );

    let start = Instant::now();
    let blob_data = vec![1u8; req.payload_size];
    let it_per_thread = req.iterations / req.worker_count;

    let mut handles = vec![];

    // 2. SPAWN THREADS
    for id in 0..req.worker_count {
        let t_it = it_per_thread;
        let t_type = req.test_type.clone();
        let t_blob = blob_data.clone();
        let payload_size = req.payload_size;

        handles.push(thread::spawn(move || -> Result<WorkerResult> {
            // --- CONNECT VIA ROUTER ---
            // We ask for "worker". The CLI Router finds where it is (local or remote).
            let mut client = CellClient::connect("worker")
                .expect("Failed to connect to worker service via Router");

            let worker_req = WorkerBenchmarkRequest {
                worker_id: id,
                iterations: t_it,
                payload_size,
                test_type: t_type,
                data_blob: t_blob,
            };

            // Serialize request
            let payload = cell_sdk::rkyv::to_bytes::<_, 1024>(&worker_req)?;

            // Call Worker
            let resp_bytes = client.call(&payload)?;

            // Parse Response
            let archived_resp =
                cell_sdk::rkyv::check_archived_root::<WorkerBenchmarkResponse>(&resp_bytes)?;
            let resp: WorkerBenchmarkResponse =
                archived_resp.deserialize(&mut cell_sdk::rkyv::Infallible)?;

            Ok(WorkerResult {
                worker_id: id,
                iterations_completed: resp.iterations_completed,
                duration_ms: resp.duration_ms,
                throughput: resp.throughput,
            })
        }));
    }

    // 3. COLLECT RESULTS
    let mut results = Vec::new();
    for h in handles {
        match h.join() {
            Ok(Ok(res)) => results.push(res),
            Ok(Err(e)) => println!("Thread failed: {}", e),
            Err(_) => println!("Thread panicked"),
        }
    }

    // 4. SEND TO AGGREGATOR
    println!("📊 Sending {} results to aggregator...", results.len());

    let agg_req = AggregateRequest {
        worker_results: results,
        test_type: req.test_type.clone(),
    };
    let agg_payload = cell_sdk::rkyv::to_bytes::<_, 4096>(&agg_req)?;

    // Connect to Aggregator via Router
    let agg_resp_bytes = cell_sdk::invoke_rpc("aggregator", &agg_payload)?;

    let archived_agg = cell_sdk::rkyv::check_archived_root::<AggregateResponse>(&agg_resp_bytes)?;
    let agg_resp: AggregateResponse = archived_agg.deserialize(&mut cell_sdk::rkyv::Infallible)?;

    println!("{}", agg_resp.summary);

    // 5. REPLY TO CLI
    let duration = start.elapsed();
    let response = BenchmarkResponse {
        total_duration_ms: duration.as_millis() as u64,
        throughput: agg_resp.total_throughput,
        summary: agg_resp.summary,
    };

    let bytes = cell_sdk::rkyv::to_bytes::<_, 1024>(&response)?;
    Ok(bytes.into_vec())
}
