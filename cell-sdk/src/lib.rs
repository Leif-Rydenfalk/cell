// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate self as cell_sdk;

pub use cell_core::{channel, CellError, Transport, Vesicle};
pub use cell_discovery as discovery;
pub use cell_macros::{cell_remote, expand, handler, protein, service};
pub use cell_model::*;
pub use cell_transport::{resolve_socket_dir, Membrane, Synapse}; // Export discovery module

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
pub mod logging;
pub mod mesh;
pub mod metrics;
pub mod runtime;
pub mod system;
pub mod test_context;
pub mod tissue;

pub mod prelude {
    pub use super::serde::{Deserialize, Serialize};
    pub use super::{
        anyhow::{Error, Result},
        cell_remote,
        config::CellConfig,
        discovery, expand, handler, protein,
        runtime::Runtime,
        service, Synapse,
    };
}
