use anyhow::Result;
use cell_consensus::{ConsensusConfig, RaftNode, StateMachine};
use serial_test::serial;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

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
#[serial]
async fn test_3_node_cluster_broadcast() -> Result<()> {
    let dir = tempdir()?;

    // --- Configs (Ports 10010, 10011, 10012) ---
    let peers_all = vec![
        "127.0.0.1:10010".to_string(),
        "127.0.0.1:10011".to_string(),
        "127.0.0.1:10012".to_string(),
    ];

    // Helper to spawn node
    async fn spawn_node(
        id: u64,
        peers: Vec<String>,
        dir: &std::path::Path,
    ) -> (Arc<RaftNode>, Arc<MockSM>) {
        let sm = Arc::new(MockSM::new());
        let config = ConsensusConfig {
            id,
            peers,
            storage_path: dir.join(format!("node{}.wal", id)),
        };
        let node = RaftNode::new(config, sm.clone())
            .await
            .expect("Failed to spawn node");
        (node, sm)
    }

    // --- Spawn Nodes ---
    // In this "All-Write" mesh, everyone peers with everyone for simplicity,
    // or at least Leader (10) needs to know about 11 and 12.
    // For this test, let's make Node 10 broadcast to 11 and 12.
    let (node10, sm10) = spawn_node(
        10,
        vec!["127.0.0.1:10011".into(), "127.0.0.1:10012".into()],
        dir.path(),
    )
    .await;
    let (_node11, sm11) = spawn_node(11, vec![], dir.path()).await; // Passive
    let (_node12, sm12) = spawn_node(12, vec![], dir.path()).await; // Passive

    tokio::time::sleep(Duration::from_millis(200)).await;

    // --- Action ---
    println!("Node 10 Broadcasting 'ClusterMessage'...");
    node10.propose(b"ClusterMessage".to_vec()).await?;

    // --- Verify ---
    // Wait for replication
    let start = std::time::Instant::now();
    loop {
        if sm11.get_all().len() == 1 && sm12.get_all().len() == 1 {
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            panic!(
                "Replication timeout. Node 11: {:?}, Node 12: {:?}",
                sm11.get_all(),
                sm12.get_all()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(sm10.get_all()[0], "ClusterMessage");
    assert_eq!(sm11.get_all()[0], "ClusterMessage");
    assert_eq!(sm12.get_all()[0], "ClusterMessage");

    Ok(())
}
