pub mod wal;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;
use wal::{LogEntry, WriteAheadLog};

pub trait StateMachine: Send + Sync + 'static {
    fn apply(&self, command: &[u8]);
}

pub struct RaftConfig {
    pub id: u64,
    pub storage_path: std::path::PathBuf,
}

/// A simplified Leaderless/All-Write Consensus engine for MVP.
/// Real Raft requires voting/terms, but for the Cell "Mycelium",
/// we start with distributed logging + local application.
pub struct RaftNode {
    wal: Arc<Mutex<WriteAheadLog>>,
    state_machine: Arc<dyn StateMachine>,
    commit_index: Arc<Mutex<u64>>,
}

impl RaftNode {
    pub async fn new(config: RaftConfig, sm: Arc<dyn StateMachine>) -> Result<Arc<Self>> {
        // 1. Recover State
        let mut wal = WriteAheadLog::open(&config.storage_path)?;
        let entries = wal.read_all()?;
        println!("[Raft] Replaying {} entries...", entries.len());
        
        for entry in &entries {
            if let LogEntry::Command(data) = entry {
                sm.apply(data);
            }
        }

        Ok(Arc::new(Self {
            wal: Arc::new(Mutex::new(wal)),
            state_machine: sm,
            commit_index: Arc::new(Mutex::new(entries.len() as u64)),
        }))
    }

    /// The application calls this to propose a change.
    /// In full Raft, this sends to Leader.
    /// In MVP, we write locally and assume the Network layer broadcasts it.
    pub async fn propose(&self, command: Vec<u8>) -> Result<u64> {
        let entry = LogEntry::Command(command.clone());
        
        // 1. Persist
        {
            let mut w = self.wal.lock().await;
            w.append(&entry)?;
        }

        // 2. Apply
        self.state_machine.apply(&command);

        // 3. Update Index
        let mut idx = self.commit_index.lock().await;
        *idx += 1;
        
        Ok(*idx)
    }
}