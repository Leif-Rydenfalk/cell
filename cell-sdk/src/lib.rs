// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate self as cell_sdk;

pub use cell_core::*;
pub use cell_model::*;
pub use cell_model::Vesicle;

pub use cell_macros::{cell_remote, handler, protein, service, cell_macro};

#[cfg(feature = "transport")]
pub use cell_transport::{Membrane, Synapse, ShmClient, CoordinationHandler, resolve_socket_dir};

#[cfg(feature = "axon")]
pub use cell_axon::{AxonServer, AxonClient};

#[cfg(feature = "process")]
pub use cell_process::{MyceliumRoot};

pub use cell_discovery::{Discovery, LanDiscovery};

pub mod runtime;
pub use runtime::Runtime;

pub mod config;
pub use config::CellConfig;

pub mod metrics;
pub mod logging;

pub mod tissue;
pub use tissue::Tissue;

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