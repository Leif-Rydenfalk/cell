// cells/drift/src/main.rs
// SPDX-License-Identifier: MIT
// Configuration Drift Detection

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct DriftReport {
    pub cell_name: String,
    pub expected_version: u64,
    pub actual_version: u64,
    pub is_compliant: bool,
}

struct DriftState {
    reports: HashMap<String, DriftReport>,
}

#[service]
#[derive(Clone)]
struct DriftService {
    state: Arc<RwLock<DriftState>>,
}

#[handler]
impl DriftService {
    async fn report(&self, report: DriftReport) -> Result<bool> {
        let mut state = self.state.write().await;
        if !report.is_compliant {
            tracing::warn!("[Drift] DETECTED: {} (Exp: {}, Act: {})", 
                report.cell_name, report.expected_version, report.actual_version);
        }
        state.reports.insert(report.cell_name.clone(), report);
        Ok(true)
    }

    async fn check(&self, cell_name: String) -> Result<DriftReport> {
        // Mock logic: Real impl would query Config cell then query Target cell
        let state = self.state.read().await;
        if let Some(r) = state.reports.get(&cell_name) {
            Ok(DriftReport {
                cell_name: r.cell_name.clone(),
                expected_version: r.expected_version,
                actual_version: r.actual_version,
                is_compliant: r.is_compliant,
            })
        } else {
            Ok(DriftReport {
                cell_name,
                expected_version: 0,
                actual_version: 0,
                is_compliant: true,
            })
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Drift] Detector Active");
    let state = DriftState { reports: HashMap::new() };
    let service = DriftService { state: Arc::new(RwLock::new(state)) };
    service.serve("drift").await
}