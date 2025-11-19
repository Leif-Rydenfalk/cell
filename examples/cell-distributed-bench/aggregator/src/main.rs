use anyhow::Result;
use cell_sdk::*;

service_schema! {
    service: aggregator,
    request: AggregateRequest {
        worker_results: Vec<WorkerResult>,
        test_type: String,
    },
    response: AggregateResponse {
        total_throughput: f64,
        avg_latency_ms: f64,
        summary: String,
    }
}

#[derive(
    serde::Serialize,
    serde::Deserialize,
    cell_sdk::rkyv::Archive,
    cell_sdk::rkyv::Serialize,
    cell_sdk::rkyv::Deserialize,
    Debug,
    Clone,
)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct WorkerResult {
    pub worker_id: u32,
    pub iterations_completed: u32,
    pub duration_ms: u64,
    pub throughput: f64,
}

fn main() -> Result<()> {
    run_service_with_schema("aggregator", __CELL_SCHEMA__, |request_bytes| {
        // FIX: Added .map_err
        let req = cell_sdk::rkyv::check_archived_root::<AggregateRequest>(request_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid data: {}", e))?;

        println!(
            "📊 Aggregating results from {} workers...",
            req.worker_results.len()
        );

        let total_throughput: f64 = req.worker_results.iter().map(|w| w.throughput).sum();

        let avg_latency_ms: f64 = if req.worker_results.is_empty() {
            0.0
        } else {
            req.worker_results
                .iter()
                .map(|w| w.duration_ms as f64)
                .sum::<f64>()
                / req.worker_results.len() as f64
        };

        let summary = format!(
            "Benchmark '{}' Complete.\n   Nodes: {}\n   Total Throughput: {:.2} ops/sec\n   Avg Latency: {:.2} ms",
            req.test_type.as_str(),
            req.worker_results.len(),
            total_throughput,
            avg_latency_ms
        );

        let response = AggregateResponse {
            total_throughput,
            avg_latency_ms,
            summary,
        };

        let bytes = cell_sdk::rkyv::to_bytes::<_, 1024>(&response)
            .map_err(|e| anyhow::anyhow!("Serialize err: {}", e))?;
        Ok(bytes.into_vec())
    })
}
