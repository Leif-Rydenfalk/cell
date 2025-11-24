use anyhow::Result;
use cell_consensus::{ConsensusConfig, RaftNode, StateMachine};
use serial_test::serial;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

// --- Mock State Machine ---
// Captures applied commands so we can verify replication
struct MockSM {
    applied: Mutex<Vec<String>>,
}

impl MockSM {
    fn new() -> Self {
        Self {
            applied: Mutex::new(Vec::new()),
        }
    }
    fn get_all(&self) -> Vec<String> {
        self.applied.lock().unwrap().clone()
    }
}

impl StateMachine for MockSM {
    fn apply(&self, command: &[u8]) {
        if let Ok(s) = String::from_utf8(command.to_vec()) {
            self.applied.lock().unwrap().push(s);
        }
    }
}

#[tokio::test]
#[serial] // Serial because we bind specific TCP ports
async fn test_replication_node_to_node() -> Result<()> {
    let dir = tempdir()?;

    // --- Setup Node 2 (Follower/Peer) ---
    // ID 2 -> Ports 10002 (Consensus)
    let sm2 = Arc::new(MockSM::new());
    let config2 = ConsensusConfig {
        id: 2,
        peers: vec![], // Doesn't need to broadcast back for this test
        storage_path: dir.path().join("node2.wal"),
    };
    let _node2 = RaftNode::new(config2, sm2.clone()).await?;

    // Give Node 2 a moment to bind listener
    tokio::time::sleep(Duration::from_millis(100)).await;

    // --- Setup Node 1 (Leader/Proposer) ---
    // ID 1 -> Port 10001 (Consensus)
    let sm1 = Arc::new(MockSM::new());
    let config1 = ConsensusConfig {
        id: 1,
        peers: vec!["127.0.0.1:10002".to_string()], // Points to Node 2
        storage_path: dir.path().join("node1.wal"),
    };
    let node1 = RaftNode::new(config1, sm1.clone()).await?;

    // --- Action: Propose Data ---
    println!("Node 1 Proposing 'Alpha'...");
    node1.propose(b"Alpha".to_vec()).await?;

    println!("Node 1 Proposing 'Beta'...");
    node1.propose(b"Beta".to_vec()).await?;

    // --- Wait for Replication ---
    // Since broadcast is async over TCP, we poll Node 2's state machine
    let start = std::time::Instant::now();
    let mut success = false;

    while start.elapsed() < Duration::from_secs(2) {
        let entries = sm2.get_all();
        if entries.len() == 2 {
            assert_eq!(entries[0], "Alpha");
            assert_eq!(entries[1], "Beta");
            success = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(success, "Node 2 failed to receive entries within timeout");

    // Verify Node 1 also applied it locally
    let entries_1 = sm1.get_all();
    assert_eq!(entries_1.len(), 2);
    assert_eq!(entries_1[0], "Alpha");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_durability_on_restart() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("persistent.wal");

    // 1. Start Node, Write Data, Stop Node
    {
        let sm = Arc::new(MockSM::new());
        let config = ConsensusConfig {
            id: 3,
            peers: vec![],
            storage_path: path.clone(),
        };
        let node = RaftNode::new(config, sm.clone()).await?;
        node.propose(b"PersistMe".to_vec()).await?;
        // Node drops here
    }

    // 2. Restart Node with same WAL
    {
        let sm = Arc::new(MockSM::new());
        let config = ConsensusConfig {
            id: 3,
            peers: vec![],
            storage_path: path.clone(),
        };
        // During `new()`, it should replay WAL into SM
        let _node = RaftNode::new(config, sm.clone()).await?;

        // Verify SM state
        let entries = sm.get_all();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], "PersistMe");
    }

    Ok(())
}
