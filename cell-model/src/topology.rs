// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use serde::{Deserialize, Serialize};

/// The Static Identity of every node in the known universe.
/// If it's not here, it doesn't exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeId {
    // We define our cluster members explicitly.
    Alpha,
    Beta,
    Gamma,
    // Add more as the cluster grows...
}

/// The roles a cell can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Consensus,
    Ledger,
    Worker,
}

/// Immutable Configuration burned into the binary.
pub struct StaticConfig {
    pub id: NodeId,
    pub role: Role,
    // Networking is defined logically (e.g., DNS names), not raw IPs, 
    // to allow infrastructure flex while keeping the graph static.
    pub net_address: &'static str,
    pub peers: &'static [NodeId],
}

impl NodeId {
    /// The Source of Truth.
    /// This function is const. It is evaluated/inlined at compile time.
    /// There is NO parsing here.
    pub const fn get_config(&self) -> StaticConfig {
        match self {
            NodeId::Alpha => StaticConfig {
                id: NodeId::Alpha,
                role: Role::Consensus,
                net_address: "cell-alpha.local:9000",
                // Compiler guarantees these peers exist in the Enum.
                peers: &[NodeId::Beta, NodeId::Gamma],
            },
            NodeId::Beta => StaticConfig {
                id: NodeId::Beta,
                role: Role::Consensus,
                net_address: "cell-beta.local:9000",
                peers: &[NodeId::Alpha, NodeId::Gamma],
            },
            NodeId::Gamma => StaticConfig {
                id: NodeId::Gamma,
                role: Role::Consensus,
                net_address: "cell-gamma.local:9000",
                peers: &[NodeId::Alpha, NodeId::Beta],
            },
        }
    }

    /// Helper for string-to-enum conversion (The ONLY place parsing happens, at the very edge)
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Alpha" => Some(NodeId::Alpha),
            "Beta" => Some(NodeId::Beta),
            "Gamma" => Some(NodeId::Gamma),
            _ => None,
        }
    }
}