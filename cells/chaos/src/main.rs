// cells/chaos/src/main.rs
// SPDX-License-Identifier: MIT
// Failure Injection Service

use cell_sdk::*;
use anyhow::Result;
use rand::prelude::*;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct ChaosExperiment {
    pub target_cell: String,
    pub fault_type: FaultType,
    pub duration_secs: u64,
}

#[protein]
pub enum FaultType {
    Latency { delay_ms: u64 },
    PacketLoss { probability: f32 },
    Crash,
}

#[service]
#[derive(Clone)]
struct ChaosService;

#[handler]
impl ChaosService {
    async fn inject(&self, exp: ChaosExperiment) -> Result<bool> {
        match exp.fault_type {
            FaultType::Crash => {
                tracing::warn!("[Chaos] KILLING cell {}", exp.target_cell);
                // Connect via Ops and send Shutdown
            }
            FaultType::Latency { delay_ms } => {
                tracing::info!("[Chaos] Adding {}ms latency to {}", delay_ms, exp.target_cell);
                // Configure Firewall/Axon proxy to delay packets
            }
            FaultType::PacketLoss { probability } => {
                tracing::info!("[Chaos] {}% Packet loss for {}", probability * 100.0, exp.target_cell);
            }
        }
        Ok(true)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Chaos] Monkey Active");
    let service = ChaosService;
    service.serve("chaos").await
}