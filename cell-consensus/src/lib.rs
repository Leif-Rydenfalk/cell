// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell


pub mod wal;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use tokio::sync::{broadcast, mpsc, oneshot};
use wal::{LogEntry, WriteAheadLog};

#[derive(Debug)]
enum WalCmd {
    Append {
        entries: Vec<LogEntry>,
        /// Notify caller when batch is both written and fsync-ed.
        done: oneshot::Sender<Result<()>>,
    },
}


pub struct RaftConfig {
    pub id: u64,
    pub storage_path: std::path::PathBuf,
}

pub trait StateMachine: Send + Sync + 'static {
    fn apply(&self, command: &[u8]);
    fn snapshot(&self) -> Vec<u8>;
    fn restore(&self, snapshot: &[u8]);
}

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

pub struct RaftNode {
    id: u64,
    wal_tx: mpsc::UnboundedSender<WalCmd>, 
    state_machine: Arc<dyn StateMachine>,
    commit_index: AtomicU64,
    network_tx: broadcast::Sender<RaftMessage>,
}

impl RaftNode {
    pub async fn new(config: RaftConfig, sm: Arc<dyn StateMachine>) -> Result<Arc<Self>> {
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
        let (wal_tx, mut wal_rx) = mpsc::unbounded_channel::<WalCmd>();
        let mut wal = wal;          // move the opened log into the task
        tokio::spawn(async move {
            while let Some(cmd) = wal_rx.recv().await {
                if let WalCmd::Append { entries, done } = cmd {
                    let res = wal.append_batch(&entries).map_err(Into::into);
                    let _ = done.send(res);
                }
            }
        });

        let (tx, _) = broadcast::channel(100);

        Ok(Arc::new(Self {
            id: config.id,
            wal_tx,
            state_machine: sm,
            commit_index: AtomicU64::new(last_index),
            network_tx: tx,
        }))
    }

    pub async fn propose(&self, command: Vec<u8>) -> Result<u64> {
        let entry = LogEntry::Command(command.clone());
        self.append_via_channel(std::slice::from_ref(&entry)).await?;
        self.state_machine.apply(&command);
        Ok(self.commit_index.fetch_add(1, Ordering::Release))
    }

    // NEW: Batch Proposal
    pub async fn propose_batch(&self, commands: Vec<Vec<u8>>) -> Result<u64> {
        if commands.is_empty() { return Ok(0); }

        let entries: Vec<LogEntry> = commands.iter()
            .map(|c| LogEntry::Command(c.clone()))
            .collect();
            
        // 1. Group Commit to Disk
        self.append_via_channel(&entries).await?;

        // 2. Apply all to Memory
        for cmd in &commands {
            self.state_machine.apply(cmd);
        }

        // 3. Update Index
        Ok(self.commit_index.fetch_add(commands.len() as u64, Ordering::Release))
    }

    // ----- 5.  Helper: channel + oneshot for back-pressure -----
    async fn append_via_channel(&self, entries: &[LogEntry]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.wal_tx.send(WalCmd::Append {
            entries: entries.to_vec(),
            done: tx,
        })?;
        // wait until WAL task finished the fsync
        rx.await?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RaftMessage> {
        self.network_tx.subscribe()
    }
}