// SPDX-License-Identifier: MIT
// cell-sdk/src/tissue.rs

use crate::{Response, Synapse};
use anyhow::{bail, Result};
use cell_model::rkyv::ser::serializers::AllocSerializer;
use rkyv::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct Tissue {
    cell_name: String,
    synapses: Arc<RwLock<Vec<Synapse>>>,
}

impl Tissue {
    pub async fn connect(cell_name: &str) -> Result<Self> {
        let mut synapses = Vec::new();

        // 1. Try Direct Neighbor (Local Monorepo)
        if let Ok(local) = Synapse::grow(cell_name).await {
            synapses.push(local);
        }

        // 2. Future: Discovery via Gossip (requires a 'gossip' neighbor)
        // For now, we rely on 1.

        if synapses.is_empty() {
            bail!("No instances of tissue '{}' found locally.", cell_name);
        }

        Ok(Self {
            cell_name: cell_name.to_string(),
            synapses: Arc::new(RwLock::new(synapses)),
        })
    }

    /// Distribute request (Round Robin)
    pub async fn distribute<'a, Req, Resp>(
        &'a mut self,
        request: &Req,
    ) -> Result<Response<'a, Resp>>
    where
        Req: Serialize<AllocSerializer<1024>>,
        // Tissue usage requires manual deserialization by caller for now or wrapping
        // For the sake of the compiler error, we return Response<Resp> but
        // the underlying fire returns Response<()>.
        // We need to fix Synapse::fire signature in synapse.rs first (Done in step 6)
    {
        let mut guard = self.synapses.write().await;
        if guard.is_empty() {
            bail!("Tissue '{}' has no active cells", self.cell_name);
        }

        let result = {
            let synapse = guard.first_mut().unwrap();
            // Synapse::fire returns Result<Response<Vec<u8>>>
            synapse.fire(request).await
        };

        // Rotate Round Robin
        guard.rotate_left(1);

        // The macros expect Response<Resp>. We must cast or wrap.
        // Since we are moving to "Bytes In/Out", this signature is actually legacy.
        // However, to satisfy existing code, we return the Owned bytes
        // and let the caller/macro cast it.

        match result {
            Ok(resp) => {
                // Ugly unsafe cast? No.
                // We just return Response::Owned bytes. The generic <Resp> is phantom here
                // until deserialization happens.
                Ok(Response::Owned(resp.into_owned()))
            }
            Err(e) => Err(e),
        }
    }

    pub async fn broadcast<'a, Req, Resp>(
        &'a mut self,
        request: &Req,
    ) -> Vec<Result<Response<'a, Resp>>>
    where
        Req: Serialize<AllocSerializer<1024>> + Clone,
    {
        let mut guard = self.synapses.write().await;
        let mut results = Vec::new();

        for syn in guard.iter_mut() {
            let res = syn.fire(request).await;
            results.push(res.map(|r| Response::Owned(r.into_owned())));
        }
        results
    }
}
