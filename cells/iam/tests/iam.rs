use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Iam = "iam");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn iam_enforces_rbac() {
    System::spawn("iam", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("iam").await.expect("Failed to connect");
    let mut iam = Iam::Client::new(synapse);
    
    let auth = iam.login(Iam::LoginRequest {
        client_id: "admin".into(),
        client_secret: "admin123".into(),
    }).await.unwrap();
    
    let allowed = iam.check(Iam::CheckPermission {
        token: auth.token,
        resource: "database".into(),
        action: "drop".into(),
    }).await.unwrap();
    assert!(allowed); 
    
    let fail_auth = iam.login(Iam::LoginRequest {
        client_id: "finance".into(),
        client_secret: "moneyprinter".into(),
    }).await.unwrap();
    
    let denied = iam.check(Iam::CheckPermission {
        token: fail_auth.token,
        resource: "nuclear_codes".into(),
        action: "launch".into(),
    }).await.unwrap();
    assert!(!denied);
}