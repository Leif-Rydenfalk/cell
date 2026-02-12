use cell_sdk::*;
use anyhow::Result;

cell_remote!(Firewall = "firewall");

#[tokio::test]
async fn firewall_rate_limiting() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();

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