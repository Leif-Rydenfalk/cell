// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

mod wal;
mod raft;

use anyhow::Result;
use cell_sdk::{service, handler, protein, Synapse};
use cell_sdk as cell;
use std::sync::Arc;
use tracing::info;
use tokio::time::Duration;

use crate::raft::{RaftNode, RaftConfig, StateMachine};

// --- API PROTOCOL ---

#[protein]
pub struct Command {
    pub data: Vec<u8>,
}

#[protein]
pub struct ProposeResult {
    pub index: u64,
}

#[protein]
pub struct LogQuery {
    pub index: u64,
}

#[protein]
pub struct LogResult {
    pub term: u64,
    pub data: Option<Vec<u8>>,
}

// --- STATE MACHINE ---

struct SimpleStateMachine;

impl StateMachine for SimpleStateMachine {
    fn apply(&self, command: &[u8]) {
        if let Ok(s) = std::str::from_utf8(command) {
            info!("[StateMachine] Applied: {}", s);
        } else {
            info!("[StateMachine] Applied binary command, len: {}", command.len());
        }
    }
    fn take_snapshot(&self) -> Vec<u8> { vec![] }
    fn restore_snapshot(&self, _data: &[u8]) {}
}

// --- SERVICE ---

struct ConsensusState {
    raft: Arc<RaftNode>,
}

#[service]
#[derive(Clone)]
struct ConsensusService {
    state: Arc<ConsensusState>,
}

#[handler]
impl ConsensusService {
    async fn propose(&self, cmd: Command) -> Result<ProposeResult> {
        let index = self.state.raft.propose(cmd.data).await?;
        Ok(ProposeResult { index })
    }

    async fn get_log_entry(&self, query: LogQuery) -> Result<LogResult> {
        let wal = self.state.raft.wal.lock().await;
        if let Some(entry) = wal.get_entry(query.index) {
             match entry {
                 crate::wal::LogEntry::Command { term, data } => {
                     Ok(LogResult { term, data: Some(data) })
                 }
                 crate::wal::LogEntry::NoOp { term } => {
                     Ok(LogResult { term, data: None })
                 }
             }
        } else {
             Err(anyhow::anyhow!("Log index out of bounds"))
        }
    }
}

// --- MAIN ---

#[tokio::main]
async fn main() -> Result<()> {
    let identity = cell_sdk::identity::Identity::get();

    tracing_subscriber::fmt().with_target(false).init();
    
    info!("╔══════════════════════════════════════════╗");
    info!("║ CONSENSUS CELL | ID: {:<19} ║", identity.node_id);
    info!("║ Name: {:<32} ║", identity.cell_name);
    info!("╚══════════════════════════════════════════╝");
    
    let peers: Vec<String> = identity.peers.iter().map(|p| p.address.clone()).collect();
    info!("Injected Peers: {:?}", peers);

    let storage_path = std::env::current_dir()?.join(format!("raft_{}.wal", identity.node_id));

    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);
    
    let raft_config = RaftConfig {
        id: identity.node_id,
        peers: peers.clone(),
        storage_path,
        election_timeout_min: 150,
        election_timeout_max: 300,
        heartbeat_interval: 50,
    };

    let sm = Arc::new(SimpleStateMachine);
    let raft = RaftNode::ignite(raft_config, sm, tx).await?;

    let service = ConsensusService {
        state: Arc::new(ConsensusState { raft: raft.clone() }),
    };

    let peers_clone = peers.clone();
    tokio::spawn(async move {
        // Fix: Use cell_sdk which re-exports rkyv and cell_core
        use cell_sdk::rkyv;
        
        while let Some((target_idx, msg)) = rx.recv().await {
             if let Some(peer_name) = peers_clone.get(target_idx as usize) {
                 let p_name = peer_name.to_string();
                 tokio::spawn(async move {
                     if let Ok(mut syn) = Synapse::grow(&p_name).await {
                         if let Ok(bytes) = rkyv::to_bytes::<_, 1024>(&msg) {
                             let vec_bytes = bytes.into_vec();
                             if let Err(e) = syn.fire_on_channel(cell_sdk::channel::CONSENSUS, &vec_bytes).await {
                                 tracing::error!("Failed to send Raft RPC to {}: {:?}", p_name, e);
                             }
                         }
                     }
                 });
             }
        }
    });

    service.serve(&identity.cell_name).await
}