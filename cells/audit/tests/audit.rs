use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Audit = "audit");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn audit_chain_integrity() {
    System::spawn("audit", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("audit").await.expect("Failed to connect");
    let mut audit = Audit::Client::new(synapse);
    
    audit.log(Audit::AuditEvent {
        actor: "system".into(), action: "boot".into(), resource: "cpu".into(), 
        outcome: "ok".into(), metadata: "".into(), timestamp: 1
    }).await.unwrap();
    
    audit.log(Audit::AuditEvent {
        actor: "user".into(), action: "login".into(), resource: "web".into(), 
        outcome: "ok".into(), metadata: "".into(), timestamp: 2
    }).await.unwrap();
    
    let is_valid = audit.verify().await.unwrap();
    assert!(is_valid);
}