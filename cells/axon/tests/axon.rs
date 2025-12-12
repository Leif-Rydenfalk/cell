use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Axon = "axon");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn axon_gateway_mounts_remote_cell() {
    // Axon is auto-spawned by bootstrap.
    // We need a target cell to bridge.
    System::spawn("ledger-v2", None).await.unwrap();
    let _ = Synapse::grow_await("ledger-v2").await.unwrap();
    
    let mut axon = Axon::Client::connect().await.expect("Axon not running");
    
    // Ask Axon to bridge "ledger-v2"
    let resp = axon.mount("ledger-v2".into()).await.unwrap();
    
    if let Axon::BridgeResponse::Mounted { socket_path } = resp {
        assert!(std::path::Path::new(&socket_path).exists());
    } else {
        panic!("Axon failed to mount local cell: {:?}", resp);
    }
}