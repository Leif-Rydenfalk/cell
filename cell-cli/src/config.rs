use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug, Clone)]
pub struct Genome {
    pub genome: Option<CellTraits>,
    #[serde(default)]
    pub axons: HashMap<String, String>,
    #[serde(default)]
    pub junctions: HashMap<String, String>,
    #[serde(default)]
    pub sources: HashMap<String, String>,
    pub workspace: Option<WorkspaceTraits>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct WorkspaceTraits {
    pub members: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CellTraits {
    pub name: String,
    #[serde(default)]
    pub listen: Option<String>,
    #[serde(default)]
    pub replicas: Option<u32>,
}
