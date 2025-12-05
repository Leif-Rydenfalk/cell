// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate self as cell_sdk;

pub use cell_model::*;
pub use cell_model::vesicle::Vesicle;

pub use cell_macros::{cell_remote, handler, protein, service};

#[cfg(feature = "transport")]
pub use cell_transport::{Membrane, Synapse, ShmClient, resolve_socket_dir};

#[cfg(feature = "axon")]
pub use cell_axon::{AxonServer, AxonClient, LanDiscovery};

#[cfg(feature = "process")]
pub use cell_process::{MyceliumRoot};

pub use rkyv;
pub use serde;
pub use anyhow;
pub use tracing;

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