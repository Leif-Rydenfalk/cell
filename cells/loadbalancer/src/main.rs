// cells/loadbalancer/src/main.rs
// SPDX-License-Identifier: MIT
// Intelligent Traffic Distribution

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use rand::Rng;

#[protein]
pub struct Backend {
    pub address: String,
    pub weight: u32,
    pub active_connections: u32,
}

#[protein]
pub enum Strategy {
    RoundRobin,
    WeightedRandom,
    LeastConnections,
}

#[protein]
pub struct ServiceGroup {
    pub name: String,
    pub strategy: Strategy,
    pub backends: Vec<Backend>,
}

#[protein]
pub struct RouteRequest {
    pub service: String,
}

#[protein]
pub struct RouteResponse {
    pub address: Option<String>,
}

struct LbState {
    services: HashMap<String, ServiceGroup>,
    rr_counters: HashMap<String, usize>,
}

#[service]
#[derive(Clone)]
struct LoadBalancerService {
    state: Arc<RwLock<LbState>>,
}

impl LoadBalancerService {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(LbState {
                services: HashMap::new(),
                rr_counters: HashMap::new(),
            })),
        }
    }
}

#[handler]
impl LoadBalancerService {
    async fn register(&self, group: ServiceGroup) -> Result<bool> {
        let mut state = self.state.write().await;
        state.services.insert(group.name.clone(), group);
        Ok(true)
    }

    async fn get_upstream(&self, req: RouteRequest) -> Result<RouteResponse> {
        let mut state = self.state.write().await;
        
        let group = match state.services.get_mut(&req.service) {
            Some(g) => g,
            None => return Ok(RouteResponse { address: None }),
        };

        if group.backends.is_empty() {
            return Ok(RouteResponse { address: None });
        }

        let selected = match group.strategy {
            Strategy::RoundRobin => {
                let counter = state.rr_counters.entry(req.service.clone()).or_insert(0);
                let idx = *counter % group.backends.len();
                *counter += 1;
                Some(group.backends[idx].address.clone())
            },
            Strategy::WeightedRandom => {
                let total_weight: u32 = group.backends.iter().map(|b| b.weight).sum();
                if total_weight == 0 {
                    None
                } else {
                    let mut r = rand::thread_rng().gen_range(0..total_weight);
                    let mut chosen = None;
                    for b in &group.backends {
                        if r < b.weight {
                            chosen = Some(b.address.clone());
                            break;
                        }
                        r -= b.weight;
                    }
                    chosen
                }
            },
            Strategy::LeastConnections => {
                group.backends.iter()
                    .min_by_key(|b| b.active_connections)
                    .map(|b| b.address.clone())
            }
        };

        Ok(RouteResponse { address: selected })
    }

    async fn update_stats(&self, service: String, address: String, active: u32) -> Result<bool> {
        let mut state = self.state.write().await;
        if let Some(group) = state.services.get_mut(&service) {
            if let Some(backend) = group.backends.iter_mut().find(|b| b.address == address) {
                backend.active_connections = active;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[LoadBalancer] Traffic Director Active");
    let service = LoadBalancerService::new();
    service.serve("loadbalancer").await
}