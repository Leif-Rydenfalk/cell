// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

pub mod axon;
pub mod discovery;
pub mod pheromones;

pub use axon::{AxonServer, AxonClient};
pub use discovery::LanDiscovery;