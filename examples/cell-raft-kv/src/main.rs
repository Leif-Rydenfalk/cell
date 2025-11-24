use anyhow::Result;
use cell_sdk::*;
use cell_consensus::{RaftNode, ConsensusConfig, StateMachine};
use dashmap::DashMap;
use std::sync::Arc;
use serde::{Serialize, Deserialize};

// --- Schema ---
signal_receptor! {
    name: kv_store,
    input: KvRequest {
        op: Op,
        key: String,
        val: Option<String>,
    },
    output: KvResponse {
        value: Option<String>,
        success: bool,
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Op {
    Get,
    Set,
}

// --- The State Machine ---
// This is the "Business Logic" that Raft protects.
struct KvStateMachine {
    store: DashMap<String, String>,
}

impl KvStateMachine {
    fn new() -> Self {
        Self { store: DashMap::new() }
    }
}

impl StateMachine for KvStateMachine {
    fn apply(&self, command: &[u8]) {
        // Deserialize the command from the log
        if let Ok(req) = bincode::deserialize::<KvRequest>(command) {
            match req.op {
                Op::Set => {
                    if let Some(v) = req.val {
                        println!("[State] Applied SET {} = {}", req.key, v);
                        self.store.insert(req.key, v);
                    }
                }
                Op::Get => { /* Read-only, no state change */ }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialize State Machine
    let state_machine = Arc::new(KvStateMachine::new());
    
    // 2. Configure Consensus (Usually loaded from genome.toml/env)
    // For demo, we hardcode ID based on args or env
    let my_id = std::env::args().nth(1).unwrap_or("1".into()).parse::<u64>()?;
    let peers = vec![
        "127.0.0.1:10002".to_string(), // Node 2 consensus port
        "127.0.0.1:10003".to_string(), // Node 3 consensus port
    ];

    let raft_config = ConsensusConfig {
        id: my_id,
        peers: peers.into_iter().filter(|_| my_id == 1).collect(), // Demo: Only Node 1 broadcasts
        storage_path: std::path::PathBuf::from(format!("run/raft-{}.wal", my_id)),
    };

    println!("[KV] Starting Node {} with WAL at {:?}", my_id, raft_config.storage_path);

    // 3. Start Raft Node
    let raft = RaftNode::new(raft_config, state_machine.clone()).await?;

    // 4. Bind Cell Membrane (RPC Interface)
    let raft_handle = raft.clone();
    let sm_handle = state_machine.clone();

    Membrane::bind(__GENOME__, move |vesicle| {
        let req = cell_sdk::rkyv::check_archived_root::<KvRequest>(vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Invalid format: {}", e))?;

        // Deserialize fully to use in Raft (rkyv is zero-copy, but Raft needs owned bytes for WAL)
        // In a polished version, we'd impl rkyv for LogEntry too.
        let req_owned: KvRequest = req.deserialize(&mut cell_sdk::rkyv::Infallible).unwrap();

        match req_owned.op {
            Op::Set => {
                // WRITE: Propose to Raft -> Wait for Commit -> Apply -> Return
                let bytes = bincode::serialize(&req_owned).unwrap();
                
                // This blocks until data is safely in WAL and (optionally) replicated
                // Note: In this simple impl, we need to wrap this in a blocking task 
                // or await it if Membrane allowed async closures (it accepts async blocks).
                futures::executor::block_on(raft_handle.propose(bytes)).unwrap();
                
                let resp = KvResponse { value: None, success: true };
                let out = cell_sdk::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
                Ok(vesicle::Vesicle::wrap(out))
            }
            Op::Get => {
                // READ: Direct from State Machine (Linearizable Read requires Raft check, skipping for MVP)
                let val = sm_handle.store.get(&req_owned.key).map(|v| v.clone());
                let resp = KvResponse { value: val, success: true };
                let out = cell_sdk::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
                Ok(vesicle::Vesicle::wrap(out))
            }
        }
    })
}