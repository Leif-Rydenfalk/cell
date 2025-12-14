use cell_sdk::*;
use anyhow::Result;

cell_remote!(Consensus = "consensus");

#[tokio::test]
async fn consensus_raft_single_node() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();

    System::spawn("consensus", None).await.expect("Failed to spawn consensus");
    let synapse = Synapse::grow_await("consensus").await.expect("Failed to connect");
    let mut c = Consensus::Client::new(synapse);
    
    let cmd = Consensus::Command { data: b"hello".to_vec() };
    let res = c.propose(cmd).await.unwrap();
    
    assert!(res.index > 0);
}