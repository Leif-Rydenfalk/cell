// cells/consensus/src/main.rs
// SPDX-License-Identifier: MIT
// Raft-based consensus with auto-discovery and leader election

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// === RAFT PROTOCOL ===

#[protein]
pub struct RaftMessage {
    pub term: u64,
    pub message: RaftMessageType,
}

#[protein]
pub enum RaftMessageType {
    RequestVote {
        candidate_id: u64,
        last_log_index: u64,
        last_log_term: u64,
    },
    VoteResponse {
        voter_id: u64,
        granted: bool,
    },
    AppendEntries {
        leader_id: u64,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    },
    AppendEntriesResponse {
        follower_id: u64,
        success: bool,
        match_index: u64,
    },
}

#[protein]
pub struct LogEntry {
    pub term: u64,
    pub index: u64,
    pub command: Vec<u8>,
}

#[protein]
pub struct ConsensusQuery {
    pub key: String,
}

#[protein]
pub struct ConsensusWrite {
    pub key: String,
    pub value: Vec<u8>,
}

#[protein]
pub struct ClusterStatus {
    pub leader_id: Option<u64>,
    pub members: Vec<NodeInfo>,
    pub term: u64,
}

#[protein]
pub struct NodeInfo {
    pub id: u64,
    pub address: String,
    pub state: NodeState,
    pub last_heartbeat: u64,
}

#[protein]
pub enum NodeState {
    Leader,
    Candidate,
    Follower,
}

// === RAFT SERVICE ===

pub struct RaftNode {
    id: u64,
    state: Arc<RwLock<State>>,
    log: Arc<RwLock<Vec<LogEntry>>>,
    peers: Arc<RwLock<HashMap<u64, String>>>,
    committed_data: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

struct State {
    current_term: u64,
    voted_for: Option<u64>,
    commit_index: u64,
    last_applied: u64,
    node_state: NodeState,
    leader_id: Option<u64>,
    last_heartbeat: std::time::Instant,
}

impl RaftNode {
    pub fn new(node_id: u64) -> Self {
        Self {
            id: node_id,
            state: Arc::new(RwLock::new(State {
                current_term: 0,
                voted_for: None,
                commit_index: 0,
                last_applied: 0,
                node_state: NodeState::Follower,
                leader_id: None,
                last_heartbeat: std::time::Instant::now(),
            })),
            log: Arc::new(RwLock::new(Vec::new())),
            peers: Arc::new(RwLock::new(HashMap::new())),
            committed_data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start_election_timer(&self) {
        let state = self.state.clone();
        let id = self.id;
        
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    300 + rand::random::<u64>() % 200
                )).await;
                
                let mut s = state.write().await;
                
                // Check if we should start election
                if !matches!(s.node_state, NodeState::Leader) {
                    let elapsed = std::time::Instant::now().duration_since(s.last_heartbeat);
                    
                    if elapsed.as_millis() > 500 {
                        println!("[Raft:{}] Election timeout, starting campaign", id);
                        s.node_state = NodeState::Candidate;
                        s.current_term += 1;
                        s.voted_for = Some(id);
                        // TODO: Send RequestVote to all peers
                    }
                }
            }
        });
    }

    pub async fn start_heartbeat(&self) {
        let state = self.state.clone();
        let id = self.id;
        let peers = self.peers.clone();
        
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                
                let s = state.read().await;
                if !matches!(s.node_state, NodeState::Leader) {
                    continue;
                }
                
                let term = s.current_term;
                drop(s);
                
                // Send heartbeat to all peers
                let peer_list = peers.read().await.clone();
                
                for (peer_id, _addr) in peer_list {
                    // TODO: Send AppendEntries (empty) as heartbeat
                    println!("[Raft:{}] Heartbeat to peer {}", id, peer_id);
                }
            }
        });
    }

    async fn append_log_entry(&self, command: Vec<u8>) -> Result<u64> {
        let mut log = self.log.write().await;
        let state = self.state.read().await;
        
        let entry = LogEntry {
            term: state.current_term,
            index: log.len() as u64,
            command,
        };
        
        let index = entry.index;
        log.push(entry);
        
        Ok(index)
    }

    async fn apply_committed(&self) {
        let state = self.state.read().await;
        let log = self.log.read().await;
        
        let last_applied = state.last_applied;
        let commit_index = state.commit_index;
        
        drop(state);
        
        if commit_index > last_applied {
            let mut data = self.committed_data.write().await;
            
            for i in (last_applied + 1)..=commit_index {
                if let Some(entry) = log.get(i as usize) {
                    // Deserialize and apply command
                    // For simplicity, assume format: "key:value"
                    if let Ok(s) = String::from_utf8(entry.command.clone()) {
                        if let Some((k, v)) = s.split_once(':') {
                            data.insert(k.to_string(), v.as_bytes().to_vec());
                        }
                    }
                }
            }
            
            self.state.write().await.last_applied = commit_index;
        }
    }

    pub async fn discover_peers(&self) {
        // Query nucleus for other consensus nodes
        if let Ok(mut nucleus) = NucleusClient::connect().await {
            if let Ok(addrs) = nucleus.discover("consensus".to_string()).await {
                let mut peers = self.peers.write().await;
                
                for (i, addr) in addrs.iter().enumerate() {
                    let peer_id = 1000 + i as u64; // Derive ID from position
                    peers.insert(peer_id, addr.clone());
                }
                
                println!("[Raft:{}] Discovered {} peers", self.id, peers.len());
            }
        }
    }
}

#[handler]
impl RaftNode {
    pub async fn handle_message(&self, msg: RaftMessage) -> Result<RaftMessage> {
        let mut state = self.state.write().await;
        
        // Update term if needed
        if msg.term > state.current_term {
            state.current_term = msg.term;
            state.voted_for = None;
            state.node_state = NodeState::Follower;
        }
        
        match msg.message {
            RaftMessageType::RequestVote { candidate_id, last_log_index, last_log_term } => {
                let granted = if msg.term >= state.current_term 
                    && (state.voted_for.is_none() || state.voted_for == Some(candidate_id)) {
                    state.voted_for = Some(candidate_id);
                    true
                } else {
                    false
                };
                
                Ok(RaftMessage {
                    term: state.current_term,
                    message: RaftMessageType::VoteResponse {
                        voter_id: self.id,
                        granted,
                    },
                })
            }
            
            RaftMessageType::AppendEntries { leader_id, prev_log_index, entries, leader_commit, .. } => {
                state.last_heartbeat = std::time::Instant::now();
                state.leader_id = Some(leader_id);
                state.node_state = NodeState::Follower;
                
                // Append entries to log
                drop(state);
                let mut log = self.log.write().await;
                
                for entry in entries {
                    if entry.index as usize == log.len() {
                        log.push(entry);
                    }
                }
                
                let match_index = log.len() as u64;
                
                Ok(RaftMessage {
                    term: self.state.read().await.current_term,
                    message: RaftMessageType::AppendEntriesResponse {
                        follower_id: self.id,
                        success: true,
                        match_index,
                    },
                })
            }
            
            _ => Ok(msg),
        }
    }

    pub async fn write(&self, req: ConsensusWrite) -> Result<bool> {
        let state = self.state.read().await;
        
        if !matches!(state.node_state, NodeState::Leader) {
            anyhow::bail!("Not the leader");
        }
        
        drop(state);
        
        // Append to log
        let command = format!("{}:{}", req.key, String::from_utf8_lossy(&req.value));
        let _index = self.append_log_entry(command.into_bytes()).await?;
        
        // Replicate to peers (simplified)
        // In real impl: wait for majority before committing
        
        self.state.write().await.commit_index += 1;
        self.apply_committed().await;
        
        Ok(true)
    }

    pub async fn read(&self, query: ConsensusQuery) -> Result<Vec<u8>> {
        let data = self.committed_data.read().await;
        
        data.get(&query.key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Key not found"))
    }

    pub async fn status(&self) -> Result<ClusterStatus> {
        let state = self.state.read().await;
        let peers = self.peers.read().await;
        
        let members: Vec<NodeInfo> = std::iter::once(NodeInfo {
            id: self.id,
            address: "self".to_string(),
            state: state.node_state.clone(),
            last_heartbeat: 0,
        })
        .chain(peers.iter().map(|(id, addr)| NodeInfo {
            id: *id,
            address: addr.clone(),
            state: NodeState::Follower,
            last_heartbeat: 0,
        }))
        .collect();
        
        Ok(ClusterStatus {
            leader_id: state.leader_id,
            members,
            term: state.current_term,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let node_id = std::env::var("NODE_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse::<u64>()?;
    
    let raft = RaftNode::new(node_id);
    
    raft.discover_peers().await;
    raft.start_election_timer().await;
    raft.start_heartbeat().await;
    
    println!("[Consensus] Raft node {} active", node_id);
    raft.serve("consensus").await
}