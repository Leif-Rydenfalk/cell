use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Consensus = "consensus-raft");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn consensus_raft_single_node() {
    System::spawn("consensus-raft", None).await.expect("Failed to spawn consensus");
    let synapse = Synapse::grow_await("consensus-raft").await.expect("Failed to connect");
    let mut c = Consensus::Client::new(synapse);
    
    let cmd = Consensus::Command { data: b"hello".to_vec() };
    let res = c.propose(cmd).await.unwrap();
    
    assert!(res.index > 0);
}