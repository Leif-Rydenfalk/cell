// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod axon;
pub mod pheromones;

pub use axon::{AxonServer, AxonClient};
// Re-export LanDiscovery from cell-discovery for backward compat if needed, 
// or implement the networking part here and feed discovery.
pub use cell_discovery::lan::LanDiscovery;