// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

mod wal;
mod compaction;
mod snapshot;
mod membership;
mod raft;

use anyhow::Result;
use cell_sdk::{service, handler, protein};
use cell_sdk as cell;
use std::sync::Arc;
use tracing::info;

// Re-export specific internal raft types for the service
use crate::raft::{RaftNode, RaftConfig, StateMachine};

#[protein]
pub struct Command {
    pub data: Vec<u8>,
}

#[protein]
pub struct ProposeResult {
    pub index: u64,
}

#[protein]
pub struct MemberInfo {
    pub id: u64,
    pub address: String,
}

struct SimpleStateMachine;

impl StateMachine for SimpleStateMachine {
    fn apply(&self, command: &[u8]) {
        // In a real system this would parse the command and update state
        info!("[StateMachine] Applied command: {} bytes", command.len());
    }
}

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
        let index = self.state.raft.propose_batch(vec![cmd.data]).await?;
        Ok(ProposeResult { index })
    }

    async fn add_member(&self, member: MemberInfo) -> Result<bool> {
        self.state.raft.add_node(member.id, member.address).await?;
        Ok(true)
    }

    async fn remove_member(&self, id: u64) -> Result<bool> {
        self.state.raft.remove_node(id).await?;
        Ok(true)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    
    // Auto-detect or default ID
    let node_id = std::env::var("CELL_NODE_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse::<u64>()?;
    
    // Auto-detect topology
    let peers: Vec<String> = std::env::var("CELL_PEERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    info!("[Consensus] Starting Raft Node {}", node_id);
    info!("[Consensus] Peers: {:?}", peers);

    // Create channel for incoming network messages handled by the Cell Runtime
    // The main loop below doesn't pump this directly; 
    // In a real impl, we would hook the service to forward consensus messages to this channel,
    // or rely on a Transport middleware. 
    // For this example, we ignite Raft which spawns its own background tasks.
    let (_tx, rx) = tokio::sync::mpsc::channel(1000);
    
    let raft_config = RaftConfig {
        id: node_id,
        storage_path: std::path::PathBuf::from(format!("./raft-{}.wal", node_id)),
        peers,
    };

    let sm = Arc::new(SimpleStateMachine);
    let raft = RaftNode::ignite(raft_config, sm, rx).await?;

    let service = ConsensusService {
        state: Arc::new(ConsensusState { raft }),
    };

    service.serve("consensus-raft").await
}