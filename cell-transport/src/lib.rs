// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

// Modules are gated to allow compilation on bare metal
#[cfg(feature = "shm")]
pub mod shm;

#[cfg(feature = "std")]
pub mod synapse;

#[cfg(feature = "std")]
pub mod membrane;

pub mod response;

#[cfg(feature = "shm")]
pub use shm::ShmClient;

#[cfg(feature = "std")]
pub use synapse::Synapse;

#[cfg(feature = "std")]
pub use membrane::Membrane;

pub use response::Response;

#[cfg(feature = "std")]
pub fn resolve_socket_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CELL_SOCKET_DIR") {
        return std::path::PathBuf::from(p);
    }
    let container_dir = std::path::Path::new("/tmp/cell");
    if container_dir.exists() {
        return container_dir.to_path_buf();
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".cell/run");
    }
    std::path::PathBuf::from("/tmp/cell")
}