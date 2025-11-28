pub mod wal;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use wal::{LogEntry, WriteAheadLog};

// --- Public Config ---

pub struct RaftConfig {
    pub id: u64,
    pub storage_path: std::path::PathBuf,
}

// --- Traits ---

pub trait StateMachine: Send + Sync + 'static {
    fn apply(&self, command: &[u8]);
    fn snapshot(&self) -> Vec<u8>;
    fn restore(&self, snapshot: &[u8]);
}

// --- Raft Protocol ---

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RaftMessage {
    AppendEntries {
        term: u64,
        leader_id: u64,
        prev_log_index: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    },
    RequestVote {
        term: u64,
        candidate_id: u64,
        last_log_index: u64,
        last_log_term: u64,
    },
    VoteResponse {
        term: u64,
        vote_granted: bool,
    },
}

// --- The Engine ---

pub struct RaftNode {
    id: u64,
    wal: Arc<Mutex<WriteAheadLog>>,
    state_machine: Arc<dyn StateMachine>,
    commit_index: Arc<Mutex<u64>>, 
    network_tx: broadcast::Sender<RaftMessage>,
}

impl RaftNode {
    pub async fn new(
        config: RaftConfig, 
        sm: Arc<dyn StateMachine>
    ) -> Result<Arc<Self>> {
        // 1. WAL Recovery
        let mut wal = WriteAheadLog::open(&config.storage_path)?;
        let entries = wal.read_all()?;
        
        if !entries.is_empty() {
            println!("[Raft] Recovering node {}. Replaying {} logs.", config.id, entries.len());
            for entry in &entries {
                if let LogEntry::Command(data) = entry {
                    sm.apply(data);
                }
            }
        }
        
        let last_index = entries.len() as u64;
        let (tx, _) = broadcast::channel(100);

        Ok(Arc::new(Self {
            id: config.id,
            wal: Arc::new(Mutex::new(wal)),
            state_machine: sm,
            commit_index: Arc::new(Mutex::new(last_index)),
            network_tx: tx,
        }))
    }

    /// Propose a new state transition (Command)
    pub async fn propose(&self, command: Vec<u8>) -> Result<u64> {
        // For MVP: We assume Leaderless / All-Write consistency (Mesh Mode)
        let entry = LogEntry::Command(command.clone());
        
        // 1. Durability (Disk)
        {
            let mut w = self.wal.lock().await;
            w.append(&entry)?;
        }

        // 2. State Machine (Memory)
        self.state_machine.apply(&command);

        // 3. Update Commit Index
        let mut idx = self.commit_index.lock().await;
        *idx += 1;
        
        // 4. Broadcast to peers (Best Effort Replication)
        let msg = RaftMessage::AppendEntries {
            term: 1,
            leader_id: self.id,
            prev_log_index: *idx - 1,
            entries: vec![entry],
            leader_commit: *idx,
        };
        let _ = self.network_tx.send(msg);

        Ok(*idx)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RaftMessage> {
        self.network_tx.subscribe()
    }
}