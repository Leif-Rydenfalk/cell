// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#[cfg(feature = "shm")]
pub mod shm;

pub mod transport;
pub mod response;

// New Module
pub mod gap_junction;

#[cfg(feature = "std")]
pub mod synapse;

#[cfg(feature = "std")]
pub mod membrane;

#[cfg(feature = "std")]
pub mod coordination;

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
#[cfg(feature = "shm")]
pub use shm::ShmClient;

pub use gap_junction::GapJunction;

pub use cell_discovery::resolve_socket_dir;