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
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Debug,
    Clone,
)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
struct WorkerResult {
    worker_id: u32,
    iterations_completed: u32,
    duration_ms: u64,
    throughput: f64,
}

fn main() -> Result<()> {
    run_service_with_schema("aggregator", __CELL_SCHEMA__, |request_bytes| {
        // ZERO-COPY validation
        let req = cell_sdk::rkyv::check_archived_root::<AggregateRequest>(request_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid data: {}", e))?;

        // req is &ArchivedAggregateRequest

        println!(
            "📊 Aggregating results from {} workers",
            req.worker_results.len()
        );

        let total_iterations: u32 = req
            .worker_results
            .iter()
            .map(|w| w.iterations_completed)
            .sum();

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

        // Accessing strings in rkyv requires as_str()
        let mut summary = format!(
            "Distributed benchmark '{}' completed:\n",
            req.test_type.as_str()
        );
        summary.push_str(&format!("   Total iterations: {}\n", total_iterations));
        summary.push_str(&format!(
            "   Combined throughput: {:.2} ops/sec\n",
            total_throughput
        ));

        for worker in req.worker_results.iter() {
            summary.push_str(&format!(
                "     Worker {}: {} iterations, {:.2} ops/sec, {} ms\n",
                worker.worker_id,
                worker.iterations_completed,
                worker.throughput,
                worker.duration_ms
            ));
        }

        let response = AggregateResponse {
            total_throughput,
            avg_latency_ms,
            summary,
        };

        let bytes = cell_sdk::rkyv::to_bytes::<_, 1024>(&response)?;
        Ok(bytes.into_vec())
    })
}
