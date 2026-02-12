// cells/metrics/src/main.rs
// SPDX-License-Identifier: MIT
// Time-Series Metrics Aggregator

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct MetricPoint {
    pub name: String,
    pub value: f64,
    pub timestamp: u64,
    pub tags: Vec<(String, String)>,
}

#[protein]
pub struct QueryRange {
    pub name: String,
    pub start: u64,
    pub end: u64,
}

struct MetricsState {
    series: HashMap<String, Vec<MetricPoint>>,
}

#[service]
#[derive(Clone)]
struct MetricsService {
    state: Arc<RwLock<MetricsState>>,
}

#[handler]
impl MetricsService {
    async fn push(&self, points: Vec<MetricPoint>) -> Result<bool> {
        let mut state = self.state.write().await;
        for p in points {
            state.series.entry(p.name.clone()).or_insert_with(Vec::new).push(p);
        }
        Ok(true)
    }

    async fn query(&self, req: QueryRange) -> Result<Vec<MetricPoint>> {
        let state = self.state.read().await;
        if let Some(data) = state.series.get(&req.name) {
            let res = data.iter()
                .filter(|p| p.timestamp >= req.start && p.timestamp <= req.end)
                .map(|p| MetricPoint {
                    name: p.name.clone(),
                    value: p.value,
                    timestamp: p.timestamp,
                    tags: p.tags.clone(),
                })
                .collect();
            Ok(res)
        } else {
            Ok(vec![])
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Metrics] TSDB Active");
    let state = MetricsState { series: HashMap::new() };
    let service = MetricsService { state: Arc::new(RwLock::new(state)) };
    service.serve("metrics").await
}