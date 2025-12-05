// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate self as cell_sdk;

// 1. Core Model (Types, Vesicle, Protocol, resolve_socket_dir)
pub use cell_model::*;
pub use cell_model::vesicle::Vesicle;

// 2. Macros
pub use cell_macros::{cell_remote, handler, protein, service};

// 3. Transport (Runtime)
#[cfg(feature = "transport")]
pub use cell_transport::{Membrane, Synapse, ShmClient};

// 4. Network (Axon)
#[cfg(feature = "axon")]
pub use cell_axon::{AxonServer, AxonClient, LanDiscovery};

// 5. Process (Lifecycle)
#[cfg(feature = "process")]
pub use cell_process::{MyceliumRoot};

// 6. Re-exports for dependencies/macros
pub use rkyv;
pub use serde;
pub use anyhow;
pub use tracing;

// Helper for rkyv validation (Used by macros)
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