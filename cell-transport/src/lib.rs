// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod circuit_breaker;
pub mod coordination;
pub mod deadline;
pub mod membrane;
pub mod pool;
pub mod response;
pub mod retry;
pub mod synapse;
pub mod transport;

#[cfg(feature = "shm")]
pub mod shm;

pub use membrane::Membrane;
pub use response::Response;
pub use synapse::Synapse;
pub use transport::UnixTransport;

// Delegate to discovery to ensure single source of truth for "system" default
pub fn resolve_socket_dir() -> std::path::PathBuf {
    cell_discovery::resolve_socket_dir()
}
