pub mod wal;
pub mod network;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex};
use wal::WriteAheadLog;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LogEntry {
    /// A standard data command applied to the state machine
    Command(Vec<u8>),
    /// Configuration change (e.g., adding a peer)
    ConfigChange,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConsensusConfig {
    pub id: u64,
    pub peers: Vec<String>, // TCP addresses of peers
    pub storage_path: std::path::PathBuf,
}

pub trait StateMachine: Send + Sync + 'static {
    fn apply(&self, command: &[u8]);
}

pub struct RaftNode {
    config: ConsensusConfig,
    wal: Arc<Mutex<WriteAheadLog>>,
    commit_index: Arc<Mutex<u64>>,
    network: network::RaftNetwork,
    state_machine: Arc<dyn StateMachine>,
}

impl RaftNode {
    pub async fn new(config: ConsensusConfig, state_machine: Arc<dyn StateMachine>) -> Result<Arc<Self>> {
        let wal = WriteAheadLog::open(&config.storage_path)?;
        let network = network::RaftNetwork::new(config.id, config.peers.clone()).await?;

        let node = Arc::new(Self {
            config,
            wal: Arc::new(Mutex::new(wal)),
            commit_index: Arc::new(Mutex::new(0)),
            network,
            state_machine,
        });

        // Background Replication Task
        let node_clone = node.clone();
        tokio::spawn(async move {
            node_clone.run_replication_loop().await;
        });

        // Replay WAL on startup
        node.replay_log().await?;

        Ok(node)
    }

    async fn replay_log(&self) -> Result<()> {
        let mut wal = self.wal.lock().await;
        let entries = wal.read_all()?;
        println!("[Raft] Replaying {} entries from WAL...", entries.len());
        
        let mut idx = 0;
        for entry in entries {
            if let LogEntry::Command(data) = entry {
                self.state_machine.apply(&data);
            }
            idx += 1;
        }
        *self.commit_index.lock().await = idx;
        Ok(())
    }

    /// Propose a new entry to the cluster.
    /// In a real Raft, this would forward to Leader. 
    /// Here we assume a simplified "Leaderless/All-Write" or "Static Leader" for the prototype.
    pub async fn propose(&self, data: Vec<u8>) -> Result<()> {
        let entry = LogEntry::Command(data.clone());

        // 1. Write to Local WAL (Durability)
        {
            let mut wal = self.wal.lock().await;
            wal.append(&entry)?;
        }

        // 2. Replicate to Peers (Consistency)
        // In full Raft, we wait for Majority Quorum. 
        // Here, we do a naive "Best Effort Broadcast" to demonstrate the mechanic.
        self.network.broadcast(entry).await?;

        // 3. Apply to State Machine
        self.state_machine.apply(&data);
        
        // 4. Update Commit Index
        let mut idx = self.commit_index.lock().await;
        *idx += 1;

        Ok(())
    }

    async fn run_replication_loop(&self) {
        let mut rx = self.network.listen();
        while let Ok(msg) = rx.recv().await {
            // Handle incoming replication requests from peers
            // In a real Raft, this handles AppendEntries
            if let LogEntry::Command(data) = msg {
                 // Write to WAL
                let mut wal = self.wal.lock().await;
                if let Err(e) = wal.append(&LogEntry::Command(data.clone())) {
                    eprintln!("[Raft] Failed to persist incoming log: {}", e);
                    continue;
                }
                // Apply
                self.state_machine.apply(&data);
            }
        }
    }
}