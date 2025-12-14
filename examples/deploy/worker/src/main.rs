// deploy/worker/src/main.rs
use cell_sdk::*;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct Task {
    pub id: String,
    pub payload: Vec<u8>,
}

#[protein]
pub struct Result {
    pub worker_id: String,
    pub task_id: String,
    pub result: Vec<u8>,
    pub duration_ms: u64,
}

#[protein]
pub struct Stats {
    pub worker_id: String,
    pub tasks_completed: u64,
    pub uptime_secs: u64,
    pub cpu_percent: f32,
    pub memory_mb: u64,
}

struct WorkerState {
    worker_id: String,
    tasks_completed: u64,
    start_time: std::time::Instant,
}

#[service]
#[derive(Clone)]
struct WorkerService {
    state: Arc<RwLock<WorkerState>>,
}

#[handler]
impl WorkerService {
    // Process a task
    async fn process(&self, task: Task) -> anyhow::Result<Result> {
        let start = std::time::Instant::now();
        
        // Simulate work (hash the payload)
        let result = blake3::hash(&task.payload);
        
        // Simulate variable processing time
        tokio::time::sleep(tokio::time::Duration::from_millis(
            rand::random::<u64>() % 100
        )).await;
        
        let mut state = self.state.write().await;
        state.tasks_completed += 1;
        
        Ok(Result {
            worker_id: state.worker_id.clone(),
            task_id: task.id,
            result: result.as_bytes().to_vec(),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
    
    // Get worker statistics
    async fn stats(&self) -> anyhow::Result<Stats> {
        let state = self.state.read().await;
        
        // Get system stats (simplified)
        let cpu = rand::random::<f32>() * 50.0; // Mock
        let memory = 512; // Mock
        
        Ok(Stats {
            worker_id: state.worker_id.clone(),
            tasks_completed: state.tasks_completed,
            uptime_secs: state.start_time.elapsed().as_secs(),
            cpu_percent: cpu,
            memory_mb: memory,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .init();
    
    // Get worker ID from hostname or generate
    let worker_id = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("NODE_NAME"))
        .unwrap_or_else(|_| format!("worker-{}", rand::random::<u32>()));
    
    println!("🔧 Worker '{}' starting...", worker_id);
    
    let service = WorkerService {
        state: Arc::new(RwLock::new(WorkerState {
            worker_id,
            tasks_completed: 0,
            start_time: std::time::Instant::now(),
        })),
    };
    
    service.serve("worker").await
}