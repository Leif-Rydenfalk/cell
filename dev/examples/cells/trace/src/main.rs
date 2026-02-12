// cells/trace/src/main.rs
// SPDX-License-Identifier: MIT
// Distributed Tracing Collector

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct Span {
    pub trace_id: String,
    pub span_id: String,
    pub parent_id: Option<String>,
    pub service: String,
    pub operation: String,
    pub start_us: u64,
    pub duration_us: u64,
    pub tags: Vec<(String, String)>,
}

struct TraceState {
    traces: HashMap<String, Vec<Span>>, // TraceID -> Spans
}

#[service]
#[derive(Clone)]
struct TraceService {
    state: Arc<RwLock<TraceState>>,
}

#[handler]
impl TraceService {
    async fn push_spans(&self, spans: Vec<Span>) -> Result<bool> {
        let mut state = self.state.write().await;
        for span in spans {
            state.traces.entry(span.trace_id.clone())
                .or_insert_with(Vec::new)
                .push(span);
        }
        Ok(true)
    }

    async fn get_trace(&self, trace_id: String) -> Result<Vec<Span>> {
        let state = self.state.read().await;
        if let Some(spans) = state.traces.get(&trace_id) {
            // Need explicit clone logic due to struct ownership in Vec
            let res = spans.iter().map(|s| Span {
                trace_id: s.trace_id.clone(),
                span_id: s.span_id.clone(),
                parent_id: s.parent_id.clone(),
                service: s.service.clone(),
                operation: s.operation.clone(),
                start_us: s.start_us,
                duration_us: s.duration_us,
                tags: s.tags.clone(),
            }).collect();
            Ok(res)
        } else {
            Ok(vec![])
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Trace] Collector Active");
    let state = TraceState { traces: HashMap::new() };
    let service = TraceService { state: Arc::new(RwLock::new(state)) };
    service.serve("trace").await
}