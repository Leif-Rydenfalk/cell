use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Dht = "dht");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn dht_put_get_simple() {
    System::spawn("dht", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("dht").await.expect("Failed to connect");
    let mut d = Dht::Client::new(synapse);
    
    d.put(Dht::DhtStore { 
        key: "user:123".into(), 
        value: b"User Data".to_vec(), 
        ttl_secs: 60 
    }).await.unwrap();
    
    let val = d.get(Dht::DhtGet { key: "user:123".into() }).await.unwrap();
    
    assert_eq!(val.value, Some(b"User Data".to_vec()));
}