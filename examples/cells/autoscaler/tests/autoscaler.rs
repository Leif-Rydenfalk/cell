use cell_sdk::*;
use anyhow::Result;

cell_remote!(Autoscaler = "autoscaler");

#[tokio::test]
async fn autoscaler_logic() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();

    System::spawn("autoscaler", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("autoscaler").await.expect("Failed to connect");
    let mut a = Autoscaler::Client::new(synapse);
    
    a.register_policy(Autoscaler::ScalingPolicy {
        cell_name: "worker".into(),
        min_instances: 1,
        max_instances: 10,
        target_cpu: 50.0,
        target_memory_mb: 512,
        cooldown_secs: 5,
    }).await.unwrap();
    
    let dec = a.get_decision("worker".into()).await.unwrap();
    assert!(matches!(dec.action, Autoscaler::ScaleAction::None));
}