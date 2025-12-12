use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

// Note: Engine depends on Ledger and Consensus being available
cell_remote!(Engine = "engine");
cell_remote!(Ledger = "ledger");
// Consensus remote not needed to call Engine, but Engine uses it internally

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn engine_processes_order() {
    // 1. Start dependencies
    System::spawn("ledger", None).await.expect("Failed spawn ledger");
    let _ = Synapse::grow_await("ledger").await.expect("Failed connect ledger");
    
    System::spawn("consensus-raft", None).await.expect("Failed spawn consensus");
    let _ = Synapse::grow_await("consensus-raft").await.expect("Failed connect consensus");
    
    // 2. Start Engine
    System::spawn("engine", None).await.expect("Failed spawn engine");
    let synapse = Synapse::grow_await("engine").await.expect("Failed connect engine");
    let mut engine = Engine::Client::new(synapse);
    
    // 3. Fund user in Ledger
    let l_syn = Synapse::grow("ledger").await.unwrap();
    let mut ledger = Ledger::Client::new(l_syn);
    
    ledger.deposit(100, Ledger::Asset::USD, 1000).await.unwrap();
    
    // 4. Place Order via Engine
    let order_id = engine.place_order(
        100, 
        "BTC-USD".into(), 
        Engine::Side::Buy, 
        500, 
        1
    ).await.unwrap();
    
    assert_eq!(order_id, 12345);
    
    // 5. Verify Ledger Locked Funds
    let bal = ledger.deposit(100, Ledger::Asset::USD, 0).await.unwrap();
    assert_eq!(bal, 500); 
}