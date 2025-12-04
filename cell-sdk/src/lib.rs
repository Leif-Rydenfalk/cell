// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate self as cell_sdk;

#[cfg(feature = "axon")]
pub mod axon;
pub mod capsid;
pub mod container;
#[cfg(feature = "axon")]
pub mod discovery;
pub mod membrane;
#[cfg(feature = "axon")]
pub mod pheromones;
pub mod protocol;
pub mod ribosome;
pub mod root;
pub mod shm;
pub mod synapse;
pub mod vesicle;

pub mod bootstrap;
pub mod heartbeat;
pub mod registry;

// Re-exports for ease of use
pub use cell_macros::{cell_remote, handler, protein, service};
#[cfg(feature = "axon")]
pub use discovery::LanDiscovery;
pub use membrane::Membrane;
pub use root::MyceliumRoot;
pub use shm::ShmClient;
pub use synapse::Synapse;
pub use vesicle::Vesicle;

// Re-export dependencies used by macros
pub use rkyv;
pub use serde;

// Helper for macros
pub use membrane::resolve_socket_dir;

// Helper for rkyv validation (Fix #3)
pub fn validate_archived_root<'a, T: rkyv::Archive>(
    bytes: &'a [u8],
    context: &str,
) -> anyhow::Result<&'a T::Archived>
where
    T::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
{
    rkyv::check_archived_root::<T>(bytes).map_err(|e| {
        anyhow::anyhow!(
            "Invalid data format in {}: {:?} (len: {})",
            context,
            e,
            bytes.len()
        )
    })
}