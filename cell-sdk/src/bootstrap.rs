// SPDX-License-Identifier: MIT
use crate::synapse::Synapse;
use anyhow::{bail, Result};

/// The canonical cell-git instance (hard-coded)
pub const CELL_GIT_BOOTSTRAP: &str = "cell.network:443";

/// Alternative community instances
pub const CELL_GIT_FALLBACKS: &[&str] = &[
    "git.cell.community:443",
    "cell-git", // Local resolution via socket
    "localhost:9000",
];

pub async fn resolve_cell_git() -> Result<Synapse> {
    // 1. Try resolving locally via socket (Developer Machine / Phase 1)
    if let Ok(conn) = Synapse::grow("cell-git").await {
        return Ok(conn);
    }

    // 2. Try bootstrap endpoints (Phase 2+)
    if let Ok(conn) = Synapse::grow(CELL_GIT_BOOTSTRAP).await {
        return Ok(conn);
    }

    for fallback in CELL_GIT_FALLBACKS {
        if let Ok(conn) = Synapse::grow(fallback).await {
            return Ok(conn);
        }
    }

    bail!("Could not reach any cell-git instance")
}