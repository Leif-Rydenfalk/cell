// cell-sdk/src/lib.rs
// SPDX-License-Identifier: MIT

extern crate self as cell_sdk;

pub use cell_core::{channel, CellError, Transport, Vesicle};
pub use cell_discovery as discovery;
pub use cell_macros::{cell_remote, expand, handler, protein, service};
pub use cell_model::*;
pub use cell_transport::{Membrane, Synapse}; // Export discovery module

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

// THE NEW RESOLVER LOGIC
pub fn resolve_socket_dir() -> std::path::PathBuf {
    // 1. Env Override (Runtime Context) - Set by CLI
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return std::path::PathBuf::from(p);
    }

    // 2. Fallback (mostly for tests running without CLI)
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let instance = std::env::var("CELL_INSTANCE").unwrap_or_else(|_| "test-global".to_string());
    home.join(".cell/run").join(instance)
}
