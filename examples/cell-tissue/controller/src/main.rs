use anyhow::Result;
use cell_sdk::cell_remote;
use cell_sdk::tissue::Tissue;
use tracing::{info, error};

// --- SYMBIOSIS ---
// We define the remote cell we want to talk to.
cell_remote!(Compute = "worker");

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    std::env::set_var("CELL_LAN", "1");

    info!("--- TISSUE CONTROLLER ---");
    info!("Scanning for 'compute' cells...");

    // 1. Connect to the Tissue (Swarm)
    let mut tissue = match Tissue::connect("compute").await {
        Ok(t) => t,
        Err(_e) => {
            error!("No workers found! Run 'worker' instances first.");
            return Ok(());
        }
    };

    info!("✓ Connected to swarm.");

    // 2. UNICAST: Load Balancing
    info!("\n>>> Starting Distributed Compute (Unicast) <<<");
    for i in 1..=10 {
        let task = Compute::ComputeTask { id: i, val: i * 10 };
        
        // Wrap the task in the generated Protocol Enum
        // The macro generates 'WorkerServiceProtocol' based on the struct name 'WorkerService'
        let req = Compute::WorkerServiceProtocol::Compute { task };
        
        // .distribute() sends the request
        // We expect a WorkerServiceResponse back, not just the result struct
        let resp_wrapper = tissue.distribute::<_, Compute::WorkerServiceResponse>(&req).await?;
        let response_enum = resp_wrapper.deserialize()?;

        // Unwrap the Response Enum -> Result<ComputeResult, String> -> ComputeResult
        let result = match response_enum {
            Compute::WorkerServiceResponse::Compute(inner_result) => match inner_result {
                Ok(val) => val,
                Err(e) => {
                    error!("Task {} Failed: {}", i, e);
                    continue;
                }
            },
            _ => {
                error!("Unexpected response variant");
                continue;
            }
        };

        info!(
            "Task {:<2} -> Worker {:<4} | Result: {}", 
            i, result.worker_id, result.result
        );
    }

    // 3. MULTICAST: Broadcasting
    info!("\n>>> Broadcasting Global Update (Multicast) <<<");
    let update = Compute::StatusUpdate { msg: "System Shutdown Imminent".to_string() };
    
    // Wrap in Protocol Enum
    let req = Compute::WorkerServiceProtocol::UpdateStatus { update };
    
    // .broadcast() sends to ALL workers
    let results = tissue.broadcast::<_, Compute::WorkerServiceResponse>(&req).await;
    
    info!("Broadcast sent to {} workers.", results.len());

    Ok(())
}