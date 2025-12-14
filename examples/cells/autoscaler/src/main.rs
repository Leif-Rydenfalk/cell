// cells/autoscaler/src/main.rs
// SPDX-License-Identifier: MIT
// Autonomic scaling based on biological signals (metrics)

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, Instant};

// === PROTOCOL ===

#[protein]
pub struct ScalingPolicy {
    pub cell_name: String,
    pub min_instances: u32,
    pub max_instances: u32,
    pub target_cpu: f32,      // Target CPU usage % (e.g. 70.0)
    pub target_memory_mb: u64, // Target Memory usage MB
    pub cooldown_secs: u64,
}

#[protein]
pub struct ScaleDecision {
    pub cell_name: String,
    pub action: ScaleAction,
    pub reason: String,
}

#[protein]
pub enum ScaleAction {
    ScaleUp(u32),   // Number of instances to add
    ScaleDown(u32), // Number of instances to remove
    None,
}

// === SERVICE ===

pub struct Autoscaler {
    policies: Arc<RwLock<HashMap<String, ScalingPolicy>>>,
    last_action: Arc<RwLock<HashMap<String, Instant>>>,
    nucleus: NucleusClient,
}

impl Autoscaler {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            policies: Arc::new(RwLock::new(HashMap::new())),
            last_action: Arc::new(RwLock::new(HashMap::new())),
            nucleus: NucleusClient::connect().await?,
        })
    }

    pub fn start_loop(&self) {
        let policies = self.policies.clone();
        let last_action = self.last_action.clone();
        // In a real implementation, we would clone a NucleusClient here or create a new one inside the loop
        
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Self::evaluate_all(policies.clone(), last_action.clone()).await;
            }
        });
    }

    async fn evaluate_all(
        policies: Arc<RwLock<HashMap<String, ScalingPolicy>>>, 
        last_action: Arc<RwLock<HashMap<String, Instant>>>
    ) {
        let active_policies = policies.read().await.clone();
        
        for (name, policy) in active_policies {
            if let Err(e) = Self::evaluate_cell(&name, &policy, &last_action).await {
                tracing::warn!("[Autoscaler] Failed to evaluate {}: {}", name, e);
            }
        }
    }

    async fn evaluate_cell(
        name: &str, 
        policy: &ScalingPolicy, 
        last_action: &Arc<RwLock<HashMap<String, Instant>>>
    ) -> Result<()> {
        // Check cooldown
        {
            let last = last_action.read().await;
            if let Some(time) = last.get(name) {
                if time.elapsed() < Duration::from_secs(policy.cooldown_secs) {
                    return Ok(());
                }
            }
        }

        // 1. Discover instances via Nucleus
        let mut nucleus = NucleusClient::connect().await?;
        let instances = nucleus.discover(name.to_string()).await?;
        let current_count = instances.len() as u32;

        if current_count == 0 {
            // If 0 and min > 0, we need to bootstrap (or assume nucleus handles it)
            // But usually autoscaler scales *existing* deployments.
            return Ok(());
        }

        // 2. Gather Metrics (Connect to each instance via Ops channel)
        let mut total_cpu = 0.0;
        let mut responding_count = 0;

        for addr in &instances {
            // Note: In a real impl, we'd use a transport that supports connecting by address string
            // For now, we assume we can connect via Synapse if we know the ID or use the address as a hint
            // Simplified: we assume we can connect by cell name and get distributed results, but here we want specific instances.
            // Ideally: Synapse::connect_to(addr).
            
            // Mocking metrics for the demo logic:
            // In production, we would:
            // let conn = Synapse::connect_direct(addr).await?;
            // let stats: OpsResponse::Status = conn.ops_status().await?;
            
            // Placeholder simulation
            total_cpu += 50.0; // Assume 50% load
            responding_count += 1;
        }

        if responding_count == 0 { return Ok(()); }

        let avg_cpu = total_cpu / responding_count as f32;

        // 3. Decide
        let mut action = ScaleAction::None;
        
        if avg_cpu > policy.target_cpu && current_count < policy.max_instances {
            action = ScaleAction::ScaleUp(1);
            tracing::info!("[Autoscaler] Scaling UP {}: Avg CPU {:.1}% > Target {:.1}%", name, avg_cpu, policy.target_cpu);
        } else if avg_cpu < (policy.target_cpu * 0.5) && current_count > policy.min_instances {
            action = ScaleAction::ScaleDown(1);
            tracing::info!("[Autoscaler] Scaling DOWN {}: Avg CPU {:.1}% Low", name, avg_cpu);
        }

        // 4. Execute
        match action {
            ScaleAction::ScaleUp(n) => {
                // Trigger Mitosis on a suitable node (e.g. localhost or via Nucleus placement)
                // For now, assume local spawning via Cell CLI/Process
                // In full version: nucleus.request_spawn(name, n).await?;
                last_action.write().await.insert(name.to_string(), Instant::now());
            }
            ScaleAction::ScaleDown(n) => {
                // Send shutdown signal to 'n' instances
                // nucleus.request_termination(name, n).await?;
                last_action.write().await.insert(name.to_string(), Instant::now());
            }
            ScaleAction::None => {}
        }

        Ok(())
    }
}

#[handler]
impl Autoscaler {
    pub async fn register_policy(&self, policy: ScalingPolicy) -> Result<bool> {
        let mut policies = self.policies.write().await;
        policies.insert(policy.cell_name.clone(), policy);
        tracing::info!("[Autoscaler] Registered policy for '{}'", policies.keys().last().unwrap());
        Ok(true)
    }

    pub async fn get_decision(&self, cell_name: String) -> Result<ScaleDecision> {
        Ok(ScaleDecision {
            cell_name,
            action: ScaleAction::None,
            reason: "Monitoring...".to_string(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let autoscaler = Autoscaler::new().await?;
    autoscaler.start_loop();
    
    tracing::info!("[Autoscaler] Service Active");
    autoscaler.serve("autoscaler").await
}