use cell_sdk::*;
use anyhow::Result;

cell_remote!(Vivaldi = "vivaldi");

#[tokio::test]
async fn vivaldi_distance_calc() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();

    System::spawn("vivaldi", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("vivaldi").await.expect("Failed to connect");
    let mut v = Vivaldi::Client::new(synapse);
    
    let peer_coord = Vivaldi::Coordinate {
        vec: [10.0, 0.0, 0.0],
        height: 0.0,
        error: 0.1,
    };
    
    v.update(Vivaldi::UpdateRTT {
        node_id: "peer_node".into(),
        rtt_ms: 10.0, 
        peer_coordinate: peer_coord,
    }).await.unwrap();
    
    let route = v.route(Vivaldi::RoutingQuery {
        target_cell: "peer".into(),
        source_coordinate: None,
        max_results: 1,
    }).await.unwrap();
    assert!(!route.instances.is_empty());
}