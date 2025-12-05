// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod wal;

use anyhow::Result;
use cell_model::rkyv::{self, Archive, Serialize, Deserialize};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use tokio::sync::{broadcast, mpsc, oneshot};
use wal::{LogEntry, WriteAheadLog};
use tracing::info;
use cell_sdk::Synapse;
use cell_model::Vesicle;

// Raft Message Definition using Rkyv
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub enum RaftMessage {
    AppendEntries {
        term: u64,
        leader_id: u64,
        prev_log_index: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    },
    VoteRequest { term: u64 },
}

// ... WalCmd ...
#[derive(Debug)]
enum WalCmd {
    Append {
        entries: Vec<LogEntry>,
        done: oneshot::Sender<Result<()>>,
    },
}

pub struct RaftConfig {
    pub id: u64,
    pub storage_path: std::path::PathBuf,
    pub peers: Vec<String>, // Cell names of peers
}

pub trait StateMachine: Send + Sync + 'static {
    fn apply(&self, command: &[u8]);
}

pub struct RaftNode {
    id: u64,
    peers: Vec<String>,
    wal_tx: mpsc::UnboundedSender<WalCmd>, 
    state_machine: Arc<dyn StateMachine>,
    commit_index: AtomicU64,
    // Local broadcast (internal events)
    events_tx: broadcast::Sender<RaftMessage>,
}

impl RaftNode {
    pub async fn new(config: RaftConfig, sm: Arc<dyn StateMachine>) -> Result<Arc<Self>> {
        let mut wal = WriteAheadLog::open(&config.storage_path)?;
        let entries = wal.read_all()?;
        
        if !entries.is_empty() {
            info!("[Raft] Recovering node {}. Replaying {} logs.", config.id, entries.len());
            for entry in &entries {
                if let LogEntry::Command(data) = entry {
                    sm.apply(data);
                }
            }
        }
        
        let last_index = entries.len() as u64;
        let (wal_tx, mut wal_rx) = mpsc::unbounded_channel::<WalCmd>();
        
        tokio::spawn(async move {
            while let Some(cmd) = wal_rx.recv().await {
                if let WalCmd::Append { entries, done } = cmd {
                    let res = wal.append_batch(&entries);
                    let _ = done.send(res);
                }
            }
        });

        let (tx, _) = broadcast::channel(100);

        Ok(Arc::new(Self {
            id: config.id,
            peers: config.peers,
            wal_tx,
            state_machine: sm,
            commit_index: AtomicU64::new(last_index),
            events_tx: tx,
        }))
    }

    pub async fn propose_batch(&self, commands: Vec<Vec<u8>>) -> Result<u64> {
        let entries: Vec<LogEntry> = commands.iter().map(|c| LogEntry::Command(c.clone())).collect();
        
        // 1. Write WAL
        let (tx, rx) = oneshot::channel();
        self.wal_tx.send(WalCmd::Append { entries: entries.clone(), done: tx })?;
        rx.await??;

        // 2. Replicate to Peers (Using Synapse)
        // In real Raft this is background async. For this implementation we fire and forget or wait.
        let msg = RaftMessage::AppendEntries {
            term: 1, // Placeholder
            leader_id: self.id,
            prev_log_index: 0,
            entries: entries.clone(),
            leader_commit: 0,
        };
        
        for peer in &self.peers {
            let mut syn = Synapse::grow(peer).await?;
            // Note: Synapse.fire is typed. We need a Protocol definition for Raft.
            // But RaftMessage is defined here in the library, not via macro.
            // We use the raw transport or assume RaftMessage implements the traits needed.
            // RaftMessage derives Archive/Serialize, so it works with Synapse::fire logic IF 
            // the peer implements the handler.
            
            // This implies Raft needs to be a Cell Service.
            // For now, assume ad-hoc usage via Synapse's generic fire.
             let _ = syn.fire(&msg).await;
        }

        // 3. Apply
        for cmd in &commands { self.state_machine.apply(cmd); }

        Ok(self.commit_index.fetch_add(commands.len() as u64, Ordering::Release))
    }
}