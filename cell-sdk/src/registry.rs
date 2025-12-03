// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceRegistry {
    pub cell_name: String,
    pub version: String,
    pub instances: Vec<InstanceInfo>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub node_id: String,
    pub endpoint: String,
    pub region: Option<String>,
    pub last_heartbeat: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellManifest {
    pub name: String,
    pub fingerprint: u64,
    // Add other fields from Cell.toml as needed
}

// Helper for clients/build scripts
#[derive(Clone, Debug)]
pub struct CellSpec {
    pub git: String,
    pub tag: Option<String>,
    pub policy: Option<CellPolicy>,
}

#[derive(Clone, Debug, Default)]
pub struct CellPolicy {
    pub max_latency_ms: u64,
    pub auto_spawn: bool,
}