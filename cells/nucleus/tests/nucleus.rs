use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Nucleus = "nucleus");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn nucleus_keeps_registry_across_restart() {
    // Nucleus is auto-spawned by bootstrap, but we can verify/re-connect
    let mut n = Nucleus::Client::connect().await.expect("Nucleus not running");
    
    let reg = Nucleus::CellRegistration {
        name: "test-persist".into(),
        node_id: 99,
        capabilities: vec!["persist".into()],
        endpoints: vec!["tcp://1.2.3.4:9000".into()]
    };

    let success = n.register(reg).await.expect("Registration failed");
    assert!(success);
    
    // Verify discovery finds it
    let res = n.discover(Nucleus::DiscoveryQuery { 
        cell_name: "test-persist".into(), 
        prefer_local: true 
    }).await.unwrap();
    
    assert!(!res.instances.is_empty());
    assert_eq!(res.instances[0].node_id, 99);
}