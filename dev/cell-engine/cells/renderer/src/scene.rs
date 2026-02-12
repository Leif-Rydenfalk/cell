use crate::protocol::RemoteVertex;
use std::collections::HashMap;

pub struct Entity {
    pub mesh_id: String,
    pub position: [f32; 3],
    pub scale: [f32; 3],
    pub color: [f32; 4],
    pub model_matrix: [[f32; 4]; 4], // Computed cache
}

pub struct Scene {
    pub meshes: HashMap<String, (Vec<RemoteVertex>, Vec<u32>)>,
    pub entities: HashMap<String, Entity>,
    pub camera_pos: [f32; 3],
    pub camera_target: [f32; 3],
    pub clear_color: wgpu::Color,

    // Dirty flags to tell WGPU to re-upload buffers
    pub dirty_meshes: Vec<String>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            meshes: HashMap::new(),
            entities: HashMap::new(),
            camera_pos: [0.0, 0.0, 10.0],
            camera_target: [0.0, 0.0, 0.0],
            clear_color: wgpu::Color::BLACK,
            dirty_meshes: Vec::new(),
        }
    }
}
