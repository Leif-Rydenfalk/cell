use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Blueprint {
    #[serde(default)]
    pub resources: Vec<ResourceConfig>,
    #[serde(default)]
    pub passes: Vec<PassConfig>,
}

impl Default for Blueprint {
    fn default() -> Self {
        Self {
            resources: vec![],
            passes: vec![],
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ResourceConfig {
    #[serde(rename = "buffer")]
    Buffer {
        name: String,
        size: u64,
        #[serde(default)]
        usage: Vec<String>,
    },
    #[serde(rename = "texture")]
    Texture {
        name: String,
        width: u32,
        height: u32,
        format: String,
    },
    #[serde(rename = "camera")]
    Camera {
        name: String,
        url: String,
        width: u32,
        height: u32,
    },
    #[serde(rename = "image")]
    Image { name: String, path: String },

    #[serde(rename = "sampler")]
    Sampler {
        name: String,
        #[serde(default = "default_address_mode")]
        address_mode: String,
        #[serde(default = "default_filter_mode")]
        filter_mode: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum PassConfig {
    #[serde(rename = "compute")]
    Compute {
        name: String,
        shader: String,
        entry_point: String,
        inputs: Vec<BindConfig>,
        workgroups: [u32; 3],
        #[serde(default)]
        defines: Vec<String>,
        #[serde(default = "default_true")]
        enabled: bool,
    },
    #[serde(rename = "render")]
    Render {
        name: String,
        shader: String,
        vs_entry: String,
        fs_entry: String,
        inputs: Vec<BindConfig>,
        targets: Vec<String>,
        #[serde(default)]
        depth_target: Option<String>,
        #[serde(default = "default_topology")]
        topology: String,
        vertex_count: u32,
        #[serde(default)]
        defines: Vec<String>,
        #[serde(default = "default_true")]
        enabled: bool,
    },
    // NEW: Generic Copy Pass for Ping-Pong buffers
    #[serde(rename = "copy")]
    Copy {
        name: String,
        source: String,
        destination: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BindConfig {
    pub binding: u32,
    pub resource: String,
    #[serde(default)]
    pub writable: bool,
}

fn default_true() -> bool {
    true
}
fn default_topology() -> String {
    "TriangleList".to_string()
}
fn default_address_mode() -> String {
    "clamp".to_string()
}
fn default_filter_mode() -> String {
    "linear".to_string()
}
