// SPDX-License-Identifier: MIT
// Declarative Mesh Definition (YAML)

use serde::{Deserialize, Serialize};
// Fixed: Removed unused CellInitConfig import
use alloc::string::String;
use alloc::vec::Vec;
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MeshManifest {
    pub mesh: String,
    pub cells: Vec<CellManifest>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CellManifest {
    pub name: String,
    #[serde(default = "default_replicas")]
    pub replicas: u32,
    #[serde(default)]
    pub resources: ResourceLimits,
    #[serde(default)]
    pub placement: PlacementStrategy,
    #[serde(default)]
    pub canary: Option<CanaryConfig>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ResourceLimits {
    pub cpu: Option<f32>, // Cores
    pub mem: Option<String>, // e.g. "4Gi"
    pub gpu: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PlacementStrategy {
    pub zone: Option<String>,
    pub required_instruction_set: Option<String>, // "avx512"
    pub require_tee: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanaryConfig {
    pub weight: u32, // 0-100
    pub interval: String,
}

fn default_replicas() -> u32 { 1 }