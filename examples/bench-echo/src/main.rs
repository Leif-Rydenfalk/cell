use cell_sdk::*;
use anyhow::Result;

// Simple echo for latency testing
service_schema! {
    service: bench_echo,
    request: EchoRequest {
        data: String,
    },
    response: EchoResponse {
        data: String,
        timestamp: u64,
    }
}

fn main() -> Result<()> {
    run_service_with_schema(
        "bench_echo",
        __CELL_SCHEMA__,
        |request_json| {
            let req: EchoRequest = serde_json::from_str(request_json)?;
            
            let response = EchoResponse {
                data: req.data,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            };
            
            Ok(serde_json::to_string(&response)?)
        },
    )
}
