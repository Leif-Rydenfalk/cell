// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#[cfg(feature = "shm")]
pub mod shm;

pub mod transport;
pub mod response;

#[cfg(feature = "std")]
pub mod synapse;

#[cfg(feature = "std")]
pub mod membrane;

#[cfg(feature = "std")]
pub mod coordination;

// Resilience Modules
pub mod retry;
pub mod circuit_breaker;
pub mod deadline;
pub mod load_balancer;
pub mod pool;

pub use response::Response;

#[cfg(feature = "std")]
pub use synapse::Synapse;
#[cfg(feature = "std")]
pub use membrane::Membrane;
#[cfg(feature = "std")]
pub use coordination::CoordinationHandler;

pub use transport::{UnixTransport};
#[cfg(feature = "shm")]
pub use transport::{ShmTransport, ShmConnection};
// Re-export ShmClient for SDK usage
#[cfg(feature = "shm")]
pub use shm::ShmClient;

#[cfg(feature = "axon")]
pub use transport::QuicTransport;

pub use cell_discovery::resolve_socket_dir;