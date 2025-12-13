// SPDX-License-Identifier: MIT
// Minimal SDK - Infrastructure cells handle everything else

extern crate self as cell_sdk;

pub use cell_core::{channel, CellError, Codec, Connection, Listener, Transport, Vesicle, Wire};
pub use cell_macros::{cell_remote, cell_test, handler, protein, service};
pub use cell_model::*;

#[cfg(feature = "transport")]
pub use cell_transport::{resolve_socket_dir, Membrane, Response, Synapse};

#[cfg(feature = "transport")]
pub mod system;
#[cfg(feature = "transport")]
pub use system::System;

#[cfg(feature = "transport")]
pub mod tissue;

pub mod runtime;
pub use runtime::Runtime;

pub mod identity;
pub mod test_context;
pub use test_context::CellTestContext;

// === MESH BUILDING ===
pub mod mesh;
// === DISTRIBUTED STATE ===
pub mod crdt;

pub use cell_discovery as discovery;

pub use anyhow;
pub use rkyv;
pub use serde;
pub use tracing;

// Note: cell_transport::Synapse::connect_direct is defined in cell_transport, 
// no need to reimplement it here.

// === NUCLEUS CLIENT ===

// We declare the dependency at the module level to ensure it's generated once.
// This prevents multiple macro expansions trying to trigger the build logic simultaneously.
cell_remote!(nucleus = "nucleus");

pub struct NucleusClient {
    // The macro generates a 'nucleus::Client' struct. 
    // We wrap it here to provide the high-level SDK method names.
    _inner: nucleus::Client, 
}

impl NucleusClient {
    pub async fn connect() -> anyhow::Result<Self> {
        let inner = nucleus::Client::connect().await.map_err(|e| anyhow::anyhow!("Failed to connect to nucleus: {}", e))?;
        Ok(Self { _inner: inner })
    }

    pub async fn register(&mut self, cell_name: String, node_id: u64) -> anyhow::Result<bool> {
        let res = self._inner
            .register(nucleus::CellRegistration {
                name: cell_name,
                node_id,
                capabilities: vec![],
                endpoints: vec![],
            })
            .await;

        match res {
            Ok(val) => Ok(val),
            Err(e) => Err(anyhow::anyhow!("RPC Error: {}", e)),
        }
    }

    pub async fn discover(&mut self, cell_name: String) -> anyhow::Result<Vec<String>> {
        let res = self._inner
            .discover(nucleus::DiscoveryQuery {
                cell_name,
                prefer_local: true,
            })
            .await;

        match res {
            Ok(result) => Ok(result.instances.into_iter().map(|i| i.address).collect()),
            Err(e) => Err(anyhow::anyhow!("RPC Error: {}", e)),
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
        anyhow::anyhow!(
            "Invalid format in {}: {:?} (len: {})",
            context,
            e,
            bytes.len()
        )
    })
}