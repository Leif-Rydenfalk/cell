// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod wal;

use anyhow::Result;
use rkyv::{Archive, Serialize, Deserialize};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use tokio::sync::{broadcast, mpsc, oneshot};
use wal::{LogEntry, WriteAheadLog};
use tracing::{info, warn};
use cell_transport::Synapse;
use cell_core::channel;

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
    VoteResponse { term: u64, granted: bool },
}

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
    pub peers: Vec<String>,
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
    events_tx: broadcast::Sender<RaftMessage>,
}

impl RaftNode {
    pub async fn ignite(
        config: RaftConfig, 
        sm: Arc<dyn StateMachine>,
        mut network_rx: mpsc::Receiver<Vec<u8>>,
    ) -> Result<Arc<Self>> {
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

        let node = Arc::new(Self {
            id: config.id,
            peers: config.peers,
            wal_tx,
            state_machine: sm,
            commit_index: AtomicU64::new(last_index),
            events_tx: tx,
        });

        let node_clone = node.clone();
        tokio::spawn(async move {
            while let Some(data) = network_rx.recv().await {
                if let Err(e) = node_clone.handle_packet(&data).await {
                    warn!("[Raft] Packet Error: {}", e);
                }
            }
        });

        info!("[Raft] Online as Node {}", config.id);
        Ok(node)
    }

    async fn handle_packet(&self, data: &[u8]) -> Result<()> {
        let msg = rkyv::from_bytes::<RaftMessage>(data).map_err(|e| anyhow::anyhow!("Raft deserialization failed: {}", e))?;
        
        match msg {
            RaftMessage::AppendEntries { term: _, leader_id: _, prev_log_index: _, entries, leader_commit: _ } => {
                let (tx, rx) = oneshot::channel();
                self.wal_tx.send(WalCmd::Append { entries: entries.clone(), done: tx })?;
                rx.await??;

                for entry in entries {
                    if let LogEntry::Command(cmd) = entry {
                        self.state_machine.apply(&cmd);
                    }
                }
                
                self.commit_index.fetch_add(1, Ordering::Release);
            }
            RaftMessage::VoteRequest { term: _ } => {}
            RaftMessage::VoteResponse { .. } => {}
        }
        Ok(())
    }

    pub async fn propose_batch(&self, commands: Vec<Vec<u8>>) -> Result<u64> {
        let entries: Vec<LogEntry> = commands.iter().map(|c| LogEntry::Command(c.clone())).collect();
        
        let (tx, rx) = oneshot::channel();
        self.wal_tx.send(WalCmd::Append { entries: entries.clone(), done: tx })?;
        rx.await??;

        let msg = RaftMessage::AppendEntries {
            term: 1, 
            leader_id: self.id,
            prev_log_index: 0,
            entries: entries.clone(),
            leader_commit: 0,
        };
        
        let msg_bytes = rkyv::to_bytes::<_, 1024>(&msg)?.into_vec();
        
        for peer in &self.peers {
            if let Ok(mut syn) = Synapse::grow(peer).await {
                 let _ = syn.fire_on_channel(channel::CONSENSUS, &msg_bytes).await;
            }
        }

        for cmd in &commands { self.state_machine.apply(cmd); }

        Ok(self.commit_index.fetch_add(commands.len() as u64, Ordering::Release))
    }
}