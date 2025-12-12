use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Metrics = "metrics");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn metrics_ingest_query() {
    System::spawn("metrics", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("metrics").await.expect("Failed to connect");
    let mut m = Metrics::Client::new(synapse);
    
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    
    m.push(vec![Metrics::MetricPoint {
        name: "cpu_usage".into(), 
        value: 42.0, 
        timestamp: now, 
        tags: vec![("host".into(), "test".into())]
    }]).await.unwrap();
    
    let points = m.query(Metrics::QueryRange {
        name: "cpu_usage".into(),
        start: now - 10,
        end: now + 10,
    }).await.unwrap();
    
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].value, 42.0);
}