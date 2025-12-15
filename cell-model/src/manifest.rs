// cell-model/src/manifest.rs
// SPDX-License-Identifier: MIT

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CellManifest {
    pub package: Option<PackageMeta>, // Support standard Cargo.toml structure
    pub cell: Option<CellMeta>,       // Support explicit [cell] block

    #[serde(default)]
    pub neighbors: HashMap<String, NeighborConfig>,

    #[serde(default)]
    pub local: HashMap<String, String>,
    #[serde(default)]
    pub handlers: Vec<HandlerMeta>,
    #[serde(default)]
    pub macros: HashMap<String, String>,
    pub workspace: Option<WorkspaceMeta>,

    #[serde(default)]
    pub resources: ResourceLimits,
    #[serde(default)]
    pub placement: PlacementStrategy,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PackageMeta {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CellMeta {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum NeighborConfig {
    Path(String),
    Detailed {
        path: String,
        #[serde(default)]
        autostart: bool,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkspaceMeta {
    pub namespace: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HandlerMeta {
    pub name: String,
}

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
