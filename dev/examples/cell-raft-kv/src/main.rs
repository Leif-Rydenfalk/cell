use anyhow::Result;
use cell_sdk::*;
use cell_consensus::{RaftNode, ConsensusConfig, StateMachine};
use dashmap::DashMap;
use std::sync::Arc;

// --- Schema ---
// The signal_receptor macro generates structs that are fully compatible 
// with the protein standard internally.
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

// --- Enum Definition ---
// Using #[protein] here ensures this Enum can be:
// 1. Used in zero-copy rkyv structs
// 2. Serialized to JSON if we wanted to debug via curl/Python
#[protein]
pub enum Op {
    Get,
    Set,
}

// --- The State Machine ---
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
        // Raft Log Replay
        // We use bincode for the WAL (persisted to disk), which relies on serde.
        // #[protein] provides the Serde impls required for bincode.
        if let Ok(req) = bincode::deserialize::<KvRequest>(command) {
            match req.op {
                Op::Set => {
                    if let Some(v) = req.val {
                        println!("[State] Applied SET {} = {}", req.key, v);
                        self.store.insert(req.key, v);
                    }
                }
                Op::Get => { /* Read-only */ }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialize State Machine
    let state_machine = Arc::new(KvStateMachine::new());
    
    // 2. Configure Consensus
    let my_id = std::env::args().nth(1).unwrap_or("1".into()).parse::<u64>()?;
    let raft_config = ConsensusConfig {
        id: my_id,
        peers: vec![], // Demo mode
        storage_path: std::path::PathBuf::from(format!("run/raft-{}.wal", my_id)),
    };

    println!("[KV] Starting Node {}...", my_id);

    // 3. Start Raft Node
    let raft = RaftNode::new(raft_config, state_machine.clone()).await?;

    // 4. Bind Cell Membrane
    let raft_handle = raft.clone();
    let sm_handle = state_machine.clone();

    Membrane::bind(__GENOME__, move |vesicle| {
        // Zero-copy validation
        let req = cell_sdk::rkyv::check_archived_root::<KvRequest>(vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Invalid format: {}", e))?;

        // Deserialize for internal processing
        let req_owned: KvRequest = req.deserialize(&mut cell_sdk::rkyv::Infallible).unwrap();

        match req_owned.op {
            Op::Set => {
                // Propose to Raft
                let bytes = bincode::serialize(&req_owned).unwrap();
                futures::executor::block_on(raft_handle.propose(bytes)).unwrap();
                
                let resp = KvResponse { value: None, success: true };
                let out = cell_sdk::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
                Ok(vesicle::Vesicle::wrap(out))
            }
            Op::Get => {
                // Read State
                let val = sm_handle.store.get(&req_owned.key).map(|v| v.clone());
                let resp = KvResponse { value: val, success: true };
                let out = cell_sdk::rkyv::to_bytes::<_, 256>(&resp)?.into_vec();
                Ok(vesicle::Vesicle::wrap(out))
            }
        }
    })
}