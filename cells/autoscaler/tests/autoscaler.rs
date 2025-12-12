use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Autoscaler = "autoscaler");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn autoscaler_logic() {
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