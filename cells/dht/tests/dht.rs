use cell_sdk::*;
use anyhow::Result;

cell_remote!(Dht = "dht");

#[tokio::test]
async fn dht_put_get_simple() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();

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