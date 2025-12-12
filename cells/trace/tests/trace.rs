use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(Trace = "trace");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn trace_storage() {
    System::spawn("trace", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("trace").await.expect("Failed to connect");
    let mut t = Trace::Client::new(synapse);
    
    let span = Trace::Span {
        trace_id: "trace_1".into(),
        span_id: "span_1".into(),
        parent_id: None,
        service: "frontend".into(),
        operation: "GET /".into(),
        start_us: 1000,
        duration_us: 500,
        tags: vec![],
    };
    
    t.push_spans(vec![span]).await.unwrap();
    
    let trace = t.get_trace("trace_1".into()).await.unwrap();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].operation, "GET /");
}