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
pub use synapse::Synapse;
pub use transport::UnixTransport;

pub fn resolve_socket_dir() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    let organism = std::env::var("CELL_ORGANISM").unwrap_or_else(|_| "default".to_string());
    home.join(".cell/runtime").join(organism)
}
