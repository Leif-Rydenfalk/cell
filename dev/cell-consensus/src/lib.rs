pub mod network;
pub mod wal;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use wal::WriteAheadLog;

/// Represents an operation to be applied to the State Machine.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LogEntry {
    /// A standard data command applied to the state machine
    Command(Vec<u8>),
    /// Configuration change (e.g., adding a peer) - Placeholder for future membership changes
    ConfigChange,
}

/// Configuration for the Consensus Node
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConsensusConfig {
    /// Unique ID for this node in the cluster
    pub id: u64,
    /// List of peer TCP addresses (e.g., "127.0.0.1:10002")
    pub peers: Vec<String>,
    /// Path to the local Write-Ahead Log file
    pub storage_path: std::path::PathBuf,
}

/// The user must implement this trait to define how commands change the application state.
pub trait StateMachine: Send + Sync + 'static {
    fn apply(&self, command: &[u8]);
}

/// The Consensus Engine.
///
/// This is a simplified "All-Write" / Leaderless replication node for the Cell MVP.
/// It provides durability (WAL) and atomicity (broadcast), but full linearizability
/// (Leader Election + Quorum) requires the full Raft implementation.
pub struct RaftNode {
    config: ConsensusConfig,
    wal: Arc<Mutex<WriteAheadLog>>,
    commit_index: Arc<Mutex<u64>>,
    network: network::RaftNetwork,
    state_machine: Arc<dyn StateMachine>,
}

impl RaftNode {
    /// Initialized the Consensus Node.
    ///
    /// 1. Opens the Write-Ahead Log.
    /// 2. Replays any existing logs to the State Machine (crash recovery).
    /// 3. Binds the Network listener.
    /// 4. Spawns a background task to handle incoming replication requests.
    pub async fn new(
        config: ConsensusConfig,
        state_machine: Arc<dyn StateMachine>,
    ) -> Result<Arc<Self>> {
        // 1. Open WAL
        let wal = WriteAheadLog::open(&config.storage_path)?;
        let wal = Arc::new(Mutex::new(wal));

        // 2. Crash Recovery: Replay Log BEFORE accepting new connections
        let commit_idx = {
            let mut w = wal.lock().await;
            let entries = w.read_all()?;
            if !entries.is_empty() {
                println!(
                    "[Raft] Recovering state: Replaying {} entries from WAL...",
                    entries.len()
                );
                for entry in entries.iter() {
                    if let LogEntry::Command(data) = entry {
                        state_machine.apply(data);
                    }
                }
            }
            entries.len() as u64
        };

        // 3. Start Network Layer
        let network = network::RaftNetwork::new(config.id, config.peers.clone()).await?;

        // 4. Background Replication Loop
        // We clone components individually to avoid creating a reference cycle with Arc<RaftNode>
        let net_rx = network.listen();
        let wal_bg = wal.clone();
        let sm_bg = state_machine.clone();

        tokio::spawn(async move {
            let mut rx = net_rx;
            while let Ok(msg) = rx.recv().await {
                // Handle incoming replication from peers
                if let LogEntry::Command(data) = msg {
                    // A. Persist to Disk
                    let mut w = wal_bg.lock().await;
                    // If WAL fails, we drop the message (Basic consistency check)
                    if let Ok(_) = w.append(&LogEntry::Command(data.clone())) {
                        // B. Apply to State
                        sm_bg.apply(&data);
                    } else {
                        eprintln!("[Raft] Critical: Failed to write incoming log to disk.");
                    }
                }
            }
            // Loop terminates when RaftNetwork is dropped (Sender closed)
        });

        let node = Arc::new(Self {
            config,
            wal,
            commit_index: Arc::new(Mutex::new(commit_idx)),
            network,
            state_machine,
        });

        Ok(node)
    }

    /// Propose a new entry to the cluster.
    ///
    /// 1. Writes to local disk (Durability).
    /// 2. Broadcasts to peers (Replication).
    /// 3. Applies to local state.
    pub async fn propose(&self, data: Vec<u8>) -> Result<()> {
        let entry = LogEntry::Command(data.clone());

        // 1. Write to Local WAL
        {
            let mut wal = self.wal.lock().await;
            wal.append(&entry).context("Failed to persist to WAL")?;
        }

        // 2. Replicate to Peers (Best Effort for MVP)
        self.network.broadcast(entry).await?;

        // 3. Apply to State Machine
        self.state_machine.apply(&data);

        // 4. Update Commit Index
        let mut idx = self.commit_index.lock().await;
        *idx += 1;

        Ok(())
    }

    /// Returns the current number of entries committed to the log.
    pub async fn get_commit_index(&self) -> u64 {
        *self.commit_index.lock().await
    }
}
