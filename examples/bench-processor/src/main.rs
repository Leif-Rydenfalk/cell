use cell_sdk::*;
use anyhow::Result;

// Process large data payloads
service_schema! {
    service: bench_processor,
    request: ProcessRequest {
        numbers: Vec<f64>,
        operation: String,
    },
    response: ProcessResponse {
        result: f64,
        count: usize,
    }
}

fn main() -> Result<()> {
    run_service_with_schema(
        "bench_processor",
        __CELL_SCHEMA__,
        |request_json| {
            let req: ProcessRequest = serde_json::from_str(request_json)?;
            
            let result = match req.operation.as_str() {
                "sum" => req.numbers.iter().sum(),
                "avg" => {
                    let sum: f64 = req.numbers.iter().sum();
                    sum / req.numbers.len() as f64
                }
                "max" => req.numbers.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                "min" => req.numbers.iter().cloned().fold(f64::INFINITY, f64::min),
                _ => 0.0,
            };
            
            let response = ProcessResponse {
                result,
                count: req.numbers.len(),
            };
            
            Ok(serde_json::to_string(&response)?)
        },
    )
}
