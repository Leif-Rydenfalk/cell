// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod vesicle;
pub mod protocol;

pub use vesicle::Vesicle;
pub use protocol::*;

// Re-export for macros/dependencies
pub use rkyv;
pub use serde;

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