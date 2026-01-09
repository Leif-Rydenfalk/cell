// cell-sdk/src/lib.rs
// SPDX-License-Identifier: MIT

extern crate self as cell_sdk;

pub use cell_core::{channel, CellError, Vesicle};
pub use cell_macros::{cell_remote, expand, handler, protein, service};
pub use cell_model::*;

pub use anyhow;
pub use clap;
pub use dirs;
pub use rand;
pub use rkyv;
pub use serde;
pub use tracing;

pub mod config;
pub mod crdt;
pub mod identity;
pub mod io_client;
pub mod logging;
pub mod membrane;
pub mod mesh;
pub mod metrics;
pub mod organogenisis;
pub mod response;
pub mod runtime;
pub mod shm;
pub mod synapse;
pub mod system;
pub mod test_context;
pub mod tissue;

pub use membrane::Membrane;
pub use response::Response;
pub use synapse::Synapse;

pub mod prelude {
    pub use super::serde::{Deserialize, Serialize};
    pub use super::{
        anyhow::{Error, Result},
        cell_remote,
        config::CellConfig,
        expand, handler, protein,
        runtime::Runtime,
        service, Membrane, Synapse,
    };
}
