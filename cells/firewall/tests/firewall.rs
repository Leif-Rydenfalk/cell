use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Firewall = "firewall");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn firewall_rate_limiting() {
    System::spawn("firewall", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("firewall").await.expect("Failed to connect");
    let mut f = Firewall::Client::new(synapse);
    
    f.add_rule(Firewall::FirewallRule {
        id: "limit_me".into(),
        priority: 1,
        action: Firewall::RuleAction::Allow,
        source_cidr: "0.0.0.0/0".into(),
        destination_cell: "*".into(),
        rate_limit_rps: Some(1),
    }).await.unwrap();
    
    let req = Firewall::CheckRequest {
        source_ip: "10.0.0.1".into(),
        target_cell: "web".into(),
    };
    
    let r1 = f.check(req.clone()).await.unwrap();
    assert!(r1.allowed);
    
    let r2 = f.check(req.clone()).await.unwrap();
    assert!(!r2.allowed);
}