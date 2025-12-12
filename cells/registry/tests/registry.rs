use cell_sdk::*;
use anyhow::Result;

cell_remote!(Registry = "registry");

#[tokio::test]
async fn registry_publish_flow() {
    cell_sdk::System::ignite_local_cluster().await.unwrap();

    System::spawn("registry", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("registry").await.expect("Failed to connect");
    let mut r = Registry::Client::new(synapse);
    
    let dummy_pub_key = vec![1, 2, 3, 4];
    r.trust(Registry::TrustKey {
        author: "alice".into(),
        public_key: dummy_pub_key.clone(),
    }).await.unwrap();
    
    let pkg = Registry::Package {
        name: "my-cell".into(),
        version: "0.1.0".into(),
        description: "test".into(),
        author: "alice".into(),
        git_url: "https://github.com/alice/my-cell".into(),
        commit_hash: "abcdef".into(),
        signature: vec![0u8; 64],
    };
    
    let res = r.publish(Registry::PublishRequest {
        package: pkg,
        source_tarball: vec![],
        signing_key: dummy_pub_key,
    }).await;
    
    assert!(res.is_ok());
    
    let results = r.search(Registry::SearchQuery {
        query: "my-cell".into(),
        limit: 10,
    }).await.unwrap();
    
    assert_eq!(results.packages.len(), 1);
}