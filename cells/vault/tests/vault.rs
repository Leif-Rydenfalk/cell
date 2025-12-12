use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Vault = "vault");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn vault_secrets_lifecycle() {
    System::spawn("vault", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("vault").await.expect("Failed to connect");
    let mut v = Vault::Client::new(synapse);
    
    let secret_data = b"super_secret_payload".to_vec();
    let version = v.put(Vault::SecretWrite { 
        key: "api_key".into(), 
        value: secret_data.clone(), 
        ttl_secs: None 
    }).await.unwrap();
    
    let read_back = v.get(Vault::SecretRead { 
        key: "api_key".into(), 
        version: Some(version) 
    }).await.unwrap();
    
    assert_eq!(read_back, secret_data);
    assert!(v.rotate_keys().await.unwrap());
}