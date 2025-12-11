// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use rkyv::{Archive, Serialize, Deserialize};
use serde::{Serialize as SerdeSerialize, Deserialize as SerdeDeserialize};
use alloc::vec::Vec;
use alloc::string::String;

/// The strictly typed configuration injected into the cell at startup via the umbilical cord.
/// This replaces environment variable parsing entirely.
#[derive(Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct CellInitConfig {
    /// The unique 64-bit identity of this node in the cluster.
    pub node_id: u64,
    
    /// The human-readable name of the cell (e.g., "consensus-1").
    pub cell_name: String,
    
    /// The static topology graph known at startup.
    pub peers: Vec<PeerConfig>,
    
    /// The specific Unix socket path this cell should bind its Membrane to.
    /// Assigned by the Orchestrator/Root.
    pub socket_path: String,
}

#[derive(Archive, Serialize, Deserialize, SerdeSerialize, SerdeDeserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct PeerConfig {
    pub node_id: u64,
    pub address: String, 
}