use cell_sdk::*;

cell_remote!(Axon = "axon");

#[tokio::test]
async fn axon_gateway_mounts_remote_cell() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();

    // Axon is auto-spawned by ignite_local_cluster.
    // We need a target cell to bridge.
    System::spawn("ledger-v2", None).await.unwrap();
    let _ = Synapse::grow_await("ledger-v2").await.unwrap();
    
    let mut axon = Axon::Client::connect().await.expect("Axon not running");
    
    // Ask Axon to bridge "ledger-v2"
    let resp = axon.mount("ledger-v2".into()).await.unwrap();

    if let BridgeResponse::Mounted { socket_path } = resp {
        assert!(std::path::Path::new(&socket_path).exists());
    } else {
        panic!("Axon failed to mount local cell: {:?}", resp);
    }
}