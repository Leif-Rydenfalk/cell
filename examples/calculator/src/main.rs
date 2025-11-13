use cell_sdk::*;
use anyhow::Result;

// Define schema and generate structs
service_schema! {
    service: calculator,
    request: CalcRequest {
        operation: String,
        a: f64,
        b: f64,
    },
    response: CalcResponse {
        result: f64,
    }
}

fn main() -> Result<()> {
    run_service_with_schema(
        "calculator",
        __CELL_SCHEMA__,
        |request_json| {
            let req: CalcRequest = serde_json::from_str(request_json)?;
            
            let result = match req.operation.as_str() {
                "add" => req.a + req.b,
                "subtract" => req.a - req.b,
                "multiply" => req.a * req.b,
                "divide" => {
                    if req.b == 0.0 {
                        return Err(anyhow::anyhow!("Division by zero"));
                    }
                    req.a / req.b
                }
                _ => return Err(anyhow::anyhow!("Unknown operation")),
            };
            
            println!("  🔢 {} {} {} = {}", req.a, req.operation, req.b, result);
            
            let response = CalcResponse { result };
            Ok(serde_json::to_string(&response)?)
        },
    )
}
