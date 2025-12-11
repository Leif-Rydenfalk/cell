use anyhow::Result;
use cell_sdk::{service, handler, protein};
use cell_sdk as cell;
use tracing::{info, warn};
use std::sync::Arc;

// --- DNA ---
#[protein]
pub struct ComputeTask {
    pub id: u64,
    pub val: u64,
}

#[protein]
pub struct ComputeResult {
    pub worker_id: u64,
    pub result: u64,
}

#[protein]
pub struct StatusUpdate {
    pub msg: String,
}

// --- LOGIC ---

struct WorkerState {
    id: u64,
}

#[service]
#[derive(Clone)]
struct WorkerService {
    state: Arc<WorkerState>,
}

#[handler]
impl WorkerService {
    // Unicast Handler (Load Balanced)
    async fn compute(&self, task: ComputeTask) -> Result<ComputeResult> {
        let res = task.val.wrapping_mul(task.val);
        info!("[Worker #{}] Processed Task {}: {}^2 = {}", self.state.id, task.id, task.val, res);
        
        // Simulate work
        // tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        Ok(ComputeResult {
            worker_id: self.state.id,
            result: res,
        })
    }

    // Multicast Handler (Broadcast)
    async fn update_status(&self, update: StatusUpdate) -> Result<bool> {
        info!("[Worker #{}] RECEIVED BROADCAST: {}", self.state.id, update.msg);
        Ok(true)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Setup logging
    tracing_subscriber::fmt().with_target(false).init();

    // 2. Determine Identity (Simulating dynamic instantiation)
    // In a real deploy, this comes from env vars or the runtime.
    // Here we grab the first arg as ID, or random if missing.
    let args: Vec<String> = std::env::args().collect();
    let node_id = if args.len() > 1 {
        args[1].parse().unwrap_or(1)
    } else {
        rand::random::<u64>() % 1000
    };

    // 3. Set the Node ID environment variable so Discovery picks it up!
    // This is crucial for the Tissue feature to distinguish instances.
    std::env::set_var("CELL_NODE_ID", node_id.to_string());
    // Force LAN mode for this demo so we use the full discovery stack
    std::env::set_var("CELL_LAN", "1");

    info!("------------------------------------------------");
    info!("   TISSUE WORKER ONLINE | ID: {}", node_id);
    info!("------------------------------------------------");

    let service = WorkerService {
        state: Arc::new(WorkerState { id: node_id }),
    };

    // 4. Serve. The 'Runtime' will automatically advertise this ID via Pheromones.
    // Note: We use the species name "compute". All workers share this name.
    service.serve("compute").await
}