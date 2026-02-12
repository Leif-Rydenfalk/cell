use anyhow::Result;
use cell_consensus::{ConsensusConfig, LogEntry, RaftNode, StateMachine};
use serial_test::serial;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

struct MockSM {
    applied: Mutex<Vec<u8>>,
}
impl MockSM {
    fn new() -> Self {
        Self {
            applied: Mutex::new(Vec::new()),
        }
    }
    fn len(&self) -> usize {
        self.applied.lock().unwrap().len()
    }
}
impl StateMachine for MockSM {
    fn apply(&self, command: &[u8]) {
        self.applied.lock().unwrap().extend_from_slice(command);
    }
}

#[tokio::test]
#[serial]
async fn test_large_log_replication() -> Result<()> {
    let dir = tempdir()?;

    // Node 20 (Leader) -> Node 21 (Follower)
    let sm20 = Arc::new(MockSM::new());
    let config20 = ConsensusConfig {
        id: 20,
        peers: vec!["127.0.0.1:10021".to_string()],
        storage_path: dir.path().join("node20.wal"),
    };
    let node20 = RaftNode::new(config20, sm20.clone()).await?;

    let sm21 = Arc::new(MockSM::new());
    let config21 = ConsensusConfig {
        id: 21,
        peers: vec![],
        storage_path: dir.path().join("node21.wal"),
    };
    // FIX: Clone config21 here so we can use it later to check disk
    let _node21 = RaftNode::new(config21.clone(), sm21.clone()).await?;

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Send 100 messages
    println!("sending 100 logs...");
    for i in 0..100 {
        node20.propose(vec![i as u8]).await?;
    }

    // Wait for consistency
    let start = std::time::Instant::now();
    loop {
        if sm21.len() == 100 {
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(5) {
            panic!("Failed to replicate 100 logs. Got: {}", sm21.len());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Check WAL persistence on disk for Node 21
    let mut wal = cell_consensus::wal::WriteAheadLog::open(&config21.storage_path)?;
    let entries = wal.read_all()?;
    assert_eq!(entries.len(), 100);
    assert_eq!(entries[99], LogEntry::Command(vec![99]));

    Ok(())
}
