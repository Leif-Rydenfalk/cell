use anyhow::Result;
use cell_sdk::cell_remote;
use cell_sdk::tissue::Tissue;
use tracing::{info, error};

// --- SYMBIOSIS ---
// We define the remote cell we want to talk to.
// Note: We don't care how many there are or where they are.
// Fixed: Point to "worker" directory where the source code lives.
cell_remote!(Compute = "worker");

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    std::env::set_var("CELL_LAN", "1");

    info!("--- TISSUE CONTROLLER ---");
    info!("Scanning for 'compute' cells...");

    // 1. Connect to the Tissue (Swarm)
    // This connects to ALL available instances of "compute".
    let mut tissue = match Tissue::connect("compute").await {
        Ok(t) => t,
        Err(e) => {
            error!("No workers found! Run some worker' instances first.");
            return Ok(());
        }
    };

    info!("✓ Connected to swarm.");

    // 2. UNICAST: Load Balancing
    info!("\n>>> Starting Distributed Compute (Unicast) <<<");
    for i in 1..=10 {
        let task = Compute::ComputeTask { id: i, val: i * 10 };
        
        // .distribute() picks ONE worker (Round Robin)
        // Note: We manually deserialize here because Tissue returns raw Response wrapper
        // In the future, codegen handles this wrapper too.
        let resp_wrapper = tissue.distribute::<_, Compute::ComputeResult>(&task).await?;
        let result = resp_wrapper.deserialize()?;

        info!(
            "Task {:<2} -> Worker {:<4} | Result: {}", 
            i, result.worker_id, result.result
        );
    }

    // 3. MULTICAST: Broadcasting
    info!("\n>>> Broadcasting Global Update (Multicast) <<<");
    let update = Compute::StatusUpdate { msg: "System Shutdown Imminent".to_string() };
    
    // .broadcast() sends to ALL workers
    let results = tissue.broadcast::<_, bool>(&update).await;
    
    info!("Broadcast sent to {} workers.", results.len());

    Ok(())
}