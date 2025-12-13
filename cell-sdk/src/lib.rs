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

// NOTE: Specific system clients (Nucleus, etc.) have been removed.
// Applications needing to talk to Nucleus should declare:
// cell_remote!(Nucleus = "nucleus");