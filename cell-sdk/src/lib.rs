// cell-sdk/src/lib.rs
// SPDX-License-Identifier: MIT

extern crate self as cell_sdk;

pub use anyhow;
pub use cell_core::{channel, CellError, Vesicle};
pub use cell_macros::{cell_remote, expand, handler, protein, service};
pub use cell_model::*;
pub use clap;
pub use dirs;
pub use rand;
pub use rkyv;
pub use rkyv::{
    validation::{validators::DefaultValidator, ArchiveContext},
    CheckBytes,
};

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(check_bytes)]
pub struct ErrorResponse {
    pub code: u32,
    pub message: String,
    pub cell: String,
}

pub use serde;
pub use tracing;

pub mod config;
pub mod connection_manager;
pub mod crdt;
pub mod error;
pub mod identity;
pub mod io_client;
pub mod logging;
pub mod membrane;
pub mod mesh;
pub mod metrics;
pub mod organogenisis;
pub mod resilient_synapse; // NEW: Production-grade resilient connection
pub mod response;
pub mod runtime;
pub mod shm;
pub mod synapse; // Legacy - kept for compatibility
pub mod system;
pub mod test_context;
pub mod tissue;
pub use crate::error::*;
pub use connection_manager::{ConnectionManager, PoolConfig};

// NEW: Re-export ResilientSynapse as the primary connection type
pub use resilient_synapse::{ConnMetrics, ConnState, ResilienceConfig, ResilientSynapse};

pub use membrane::Membrane;
pub use response::Response;
// Legacy Synapse kept for backward compatibility
pub use synapse::Synapse;

pub mod prelude {
    pub use super::serde::{Deserialize, Serialize};
    pub use super::{
        anyhow::{Error, Result},
        cell_remote,
        config::CellConfig,
        expand,
        handler,
        protein,
        resilient_synapse::{ResilienceConfig, ResilientSynapse}, // NEW
        runtime::Runtime,
        service,
        Membrane,
        ResilientSynapse as Synapse, // NEW: Alias for migration
    };
}
