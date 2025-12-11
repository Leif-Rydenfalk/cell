// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::Synapse;
use anyhow::{Result, bail};
use cell_transport::load_balancer::{LoadBalancer, LoadBalanceStrategy};
use std::sync::Arc;
use tokio::sync::RwLock;
use rkyv::{Archive, Serialize};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use cell_transport::Response;

pub struct Tissue {
    cell_name: String,
    synapses: Arc<RwLock<Vec<Synapse>>>,
    balancer: Arc<LoadBalancer>,
}

impl Tissue {
    pub async fn connect(cell_name: &str) -> Result<Self> {
        let mut synapses = Vec::new();

        // 1. Try Localhost / Proxy
        // If Axon is running, it creates a local proxy socket for remote cells.
        // Synapse::grow connects to that socket transparently.
        if let Ok(local) = Synapse::grow(cell_name).await {
            synapses.push(local);
        }

        // Note: We currently only support one "instance" via the proxy socket abstraction.
        // The Axon Gateway acts as the load balancer to the remote swarm.
        // In the future, Axon could expose multiple sockets like `cell_name.1.sock`, `cell_name.2.sock`.

        if synapses.is_empty() {
            bail!("No instances of tissue '{}' found locally. Ensure Axon is bridging remote cells.", cell_name);
        }

        Ok(Self {
            cell_name: cell_name.to_string(),
            synapses: Arc::new(RwLock::new(synapses)),
            balancer: LoadBalancer::new(LoadBalanceStrategy::RoundRobin),
        })
    }

    pub async fn distribute<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let mut guard = self.synapses.write().await;
        if guard.is_empty() {
            bail!("Tissue '{}' has no active cells", self.cell_name);
        }

        let result = {
            let synapse = guard.first_mut().unwrap(); 
            synapse.fire(request).await
        };

        // Convert CellError to anyhow::Error
        let detached = result
            .map(|r| r.into_owned())
            .map_err(|e| anyhow::anyhow!("Distribution error: {}", e));
            
        guard.rotate_left(1);
        
        detached
    }

    pub async fn broadcast<'a, Req, Resp>(&'a mut self, request: &Req) -> Vec<Result<Response<'a, Resp>>>
    where
        Req: Serialize<AllocSerializer<1024>> + Clone,
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let mut guard = self.synapses.write().await;
        let mut results = Vec::new();

        for syn in guard.iter_mut() {
            let res = syn.fire(request).await;
            // Convert CellError to anyhow::Error
            results.push(
                res.map(|r| r.into_owned())
                   .map_err(|e| anyhow::anyhow!("Broadcast error: {}", e))
            );
        }
        results
    }
    
    pub async fn refresh(&self) -> Result<()> {
        Ok(())
    }
}