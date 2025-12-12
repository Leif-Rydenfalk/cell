// SPDX-License-Identifier: MIT
// Minimal SDK - Infrastructure cells handle everything else

extern crate self as cell_sdk;

// === CORE RE-EXPORTS ===
pub use cell_core::*;
pub use cell_model::*;

// === MACROS ===
pub use cell_macros::{cell_remote, handler, protein, service, cell_macro, expand};

// === TRANSPORT ===
#[cfg(feature = "transport")]
pub use cell_transport::{Membrane, Synapse, Response, resolve_socket_dir};

// === SYSTEM CLIENT (Orchestration) ===
#[cfg(feature = "transport")]
pub mod system;
#[cfg(feature = "transport")]
pub use system::System;

// === PROCESS MANAGEMENT (Daemon) ===
#[cfg(feature = "process")]
pub use cell_process as process;

// === TISSUE ===
#[cfg(feature = "transport")]
pub mod tissue;

#[cfg(all(feature = "transport", feature = "shm"))]
pub use cell_transport::ShmClient;

// === SIMPLE RUNTIME ===
pub mod runtime;
pub use runtime::Runtime;

// === IDENTITY ABSTRACTION ===
pub mod identity;

// === DISCOVERY ===
pub use cell_discovery as discovery;

// === EXTERNAL DEPS ===
pub use rkyv;
pub use serde;
pub use anyhow;
pub use tracing;

// === NUCLEUS CLIENT ===
pub struct NucleusClient {
    _synapse: Synapse,
}

impl NucleusClient {
    pub async fn connect() -> anyhow::Result<Self> {
        Ok(Self {
            _synapse: Synapse::grow("nucleus").await?,
        })
    }

    pub async fn register(&mut self, cell_name: String, node_id: u64) -> anyhow::Result<bool> {
        cell_remote!(nucleus = "nucleus");
        
        let mut client = nucleus::connect().await?;
        let res = client.register(nucleus::CellRegistration {
            name: cell_name,
            node_id,
            capabilities: vec![],
            endpoints: vec![],
        }).await;

        match res {
            Ok(val) => Ok(val),
            Err(e) => Err(anyhow::anyhow!("RPC Error: {}", e))
        }
    }

    pub async fn discover(&mut self, cell_name: String) -> anyhow::Result<Vec<String>> {
        cell_remote!(nucleus = "nucleus");
        
        let mut client = nucleus::connect().await?;
        let res = client.discover(nucleus::DiscoveryQuery {
            cell_name,
            prefer_local: true,
        }).await;

        match res {
            Ok(result) => Ok(result.instances.into_iter().map(|i| i.address).collect()),
            Err(e) => Err(anyhow::anyhow!("RPC Error: {}", e))
        }
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