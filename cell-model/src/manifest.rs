// SPDX-License-Identifier: MIT
// Declarative Mesh Definition (TOML)

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CellManifest {
    pub cell: CellMeta,
    #[serde(default)]
    pub local: HashMap<String, String>,
    #[serde(default)]
    pub handlers: Vec<HandlerMeta>,
    // Optional workspace configuration (usually in root Cell.toml)
    pub workspace: Option<WorkspaceMeta>,

    // Runtime sections (from previous YAML design, adapted to TOML structure if needed)
    #[serde(default)]
    pub resources: ResourceLimits,
    #[serde(default)]
    pub placement: PlacementStrategy,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CellMeta {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkspaceMeta {
    pub namespace: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HandlerMeta {
    pub name: String,
    // Add input/output schemas here if driven by manifest
}

// --- Runtime Structs ---

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ResourceLimits {
    pub cpu: Option<f32>,
    pub mem: Option<String>,
    pub gpu: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PlacementStrategy {
    pub zone: Option<String>,
    pub required_instruction_set: Option<String>,
    pub require_tee: bool,
}

// Kept for compatibility with older model code if any
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MeshManifest {
    pub mesh: String,
    pub cells: Vec<CellManifest>,
}
