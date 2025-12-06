// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::Synapse;
use anyhow::{Result, bail};
use cell_transport::load_balancer::{LoadBalancer, LoadBalanceStrategy};
use cell_discovery::LanDiscovery;
use std::sync::Arc;
use tokio::sync::RwLock;
use rkyv::{Archive, Serialize};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use cell_transport::Response;

/// A Tissue is a collection of identical Cells working together.
/// It acts as a client-side load balancer and swarm manager.
pub struct Tissue {
    cell_name: String,
    synapses: Arc<RwLock<Vec<Synapse>>>,
    balancer: Arc<LoadBalancer>,
}

impl Tissue {
    /// Connect to a swarm of cells by name.
    /// This discovers all available instances on LAN and Localhost and connects to them.
    pub async fn connect(cell_name: &str) -> Result<Self> {
        let mut synapses = Vec::new();

        // 1. Try Localhost (Highlander style for now, or loop through discovered sockets if naming convention supported)
        if let Ok(local) = Synapse::grow(cell_name).await {
            synapses.push(local);
        }

        // 2. Discover LAN instances
        let signals = LanDiscovery::global().find_all(cell_name).await;
        for sig in signals {
            // Avoid connecting to self if local socket connection already covered it?
            // Since we don't have perfect ID matching between local/lan yet, we might duplicate.
            // Tissue logic assumes redundancy is fine or handled by load balancer.
            
            #[cfg(feature = "axon")]
            if let Ok(syn) = Synapse::grow_to_signal(&sig).await {
                synapses.push(syn);
            }
        }

        if synapses.is_empty() {
            bail!("No instances of tissue '{}' found", cell_name);
        }

        Ok(Self {
            cell_name: cell_name.to_string(),
            synapses: Arc::new(RwLock::new(synapses)),
            balancer: LoadBalancer::new(LoadBalanceStrategy::RoundRobin),
        })
    }

    /// Distribute a request to one instance in the tissue (Unicast / Load Balanced)
    pub async fn distribute<'a, Req, Resp>(&'a mut self, request: &Req) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        // Simple Round Robin for now.
        // We need mutable access to a Synapse to fire.
        let mut guard = self.synapses.write().await;
        if guard.is_empty() {
            bail!("Tissue '{}' has no active cells", self.cell_name);
        }

        // We fake the balancer selection by just rotating the index manually or picking one
        // since Synapse requires mut access and we have a Vec.
        // A proper LoadBalancer would return an index.
        // We'll just rotate the Vec for simplicity or pick 0 and rotate.
        
        let synapse = guard.first_mut().unwrap(); 
        let res = synapse.fire(request).await;
        
        // Rotate for next time
        guard.rotate_left(1);
        
        res
    }

    /// Broadcast a request to ALL instances in the tissue (Multicast)
    /// Returns a list of results (some may fail).
    pub async fn broadcast<'a, Req, Resp>(&'a mut self, request: &Req) -> Vec<Result<Response<'a, Resp>>>
    where
        Req: Serialize<AllocSerializer<1024>> + Clone, // Req needs clone for broadcast
        Resp: Archive + 'a,
        Resp::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'static>> + 'static,
    {
        let mut guard = self.synapses.write().await;
        let mut results = Vec::new();

        for syn in guard.iter_mut() {
            results.push(syn.fire(request).await);
        }
        results
    }
    
    pub async fn refresh(&self) -> Result<()> {
        // Re-scan and add new nodes
        let signals = LanDiscovery::global().find_all(&self.cell_name).await;
        let mut guard = self.synapses.write().await;
        
        // This is a naive refresh; in production we'd check existing IDs.
        // For now, we just add new ones.
        for sig in signals {
             // Logic to check if we already have this instance would go here.
             // We lack a public ID accessor on Synapse currently.
        }
        Ok(())
    }
}