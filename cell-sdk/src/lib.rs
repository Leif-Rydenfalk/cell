// cell-sdk/src/lib.rs (CLEANED UP VERSION)
// SPDX-License-Identifier: MIT
// Minimal SDK - Infrastructure cells handle everything else

extern crate self as cell_sdk;

// === CORE RE-EXPORTS ===
pub use cell_core::*;
pub use cell_model::*;

// === MACROS ===
pub use cell_macros::{cell_remote, handler, protein, service, cell_macro, expand};

// === TRANSPORT (Required for basic RPC) ===
#[cfg(feature = "transport")]
pub use cell_transport::{Membrane, Synapse, resolve_socket_dir};

#[cfg(all(feature = "transport", feature = "shm"))]
pub use cell_transport::ShmClient;

// === AXON (Required for WAN) ===
#[cfg(feature = "axon")]
pub use cell_axon::{AxonServer, AxonClient};

// === SIMPLE RUNTIME ===
pub mod runtime;
pub use runtime::Runtime;

// === EXTERNAL DEPS ===
pub use rkyv;
pub use serde;
pub use anyhow;
pub use tracing;

// === NUCLEUS CLIENT ===
// All discovery, health checks, etc. delegated to nucleus cell

pub struct NucleusClient {
    synapse: Synapse,
}

impl NucleusClient {
    pub async fn connect() -> anyhow::Result<Self> {
        Ok(Self {
            synapse: Synapse::grow("nucleus").await?,
        })
    }

    pub async fn register(&mut self, cell_name: String, node_id: u64) -> anyhow::Result<bool> {
        cell_remote!(nucleus = "nucleus");
        
        let mut client = nucleus::connect().await?;
        client.register(nucleus::CellRegistration {
            name: cell_name,
            node_id,
            capabilities: vec![],
            endpoints: vec![],
        }).await
    }

    pub async fn discover(&mut self, cell_name: String) -> anyhow::Result<Vec<String>> {
        cell_remote!(nucleus = "nucleus");
        
        let mut client = nucleus::connect().await?;
        let result = client.discover(nucleus::DiscoveryQuery {
            cell_name,
            prefer_local: true,
        }).await?;
        
        Ok(result.instances.into_iter().map(|i| i.address).collect())
    }
}

// === VALIDATION HELPER ===
pub fn validate_archived_root<'a, T: rkyv::Archive>(
    bytes: &'a [u8],
    context: &str,
) -> anyhow::Result<&'a T::Archived>
where
    T::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
{
    rkyv::check_archived_root::<T>(bytes).map_err(|e| {
        anyhow::anyhow!("Invalid format in {}: {:?} (len: {})", context, e, bytes.len())
    })
}