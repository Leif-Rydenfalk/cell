// cell-model/src/manifest.rs
// The DNA. Defines Identity and Connectivity.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CellManifest {
    pub package: Package,
    #[serde(default)]
    pub neighbors: HashMap<String, String>, // Name -> Relative Path (e.g., "../ledger")
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
}
