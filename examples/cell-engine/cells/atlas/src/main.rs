use anyhow::Result;
use cell_sdk::*;
use rand::Rng;

#[protein(class = "RetinaContract")]
pub enum RenderCommand {
    RegisterPass {
        id: String,
        shader_source: String,
        topology: String,
    },
    UpdateResource {
        id: String,
        data: Vec<u8>,
    },
    SpawnEntity {
        id: String,
        pass_id: String,
        resource_id: String,
        vertex_count: u32,
    },
    DespawnEntity {
        id: String,
    },
    SetCamera {
        position: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
    },
    SetBackgroundColor {
        color: [f32; 4],
    },
    GetInputState,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(check_bytes)]
#[archive(crate = "cell_sdk::rkyv")]
pub struct InputState {
    pub keys_down: Vec<String>,
    pub mouse_delta: [f32; 2],
}

const CHUNK_SIZE: usize = 16;
const VOXEL_SHADER: &str = r#"
struct Camera { view_proj: mat4x4<f32>, };
@group(0) @binding(0) var<uniform> camera: Camera;
struct VertexOutput { @builtin(position) pos: vec4<f32>, @location(0) color: vec3<f32>, };
@vertex fn vs_main(@location(0) p: vec3<f32>, @location(1) c: vec3<f32>) -> VertexOutput {
    return VertexOutput(camera.view_proj * vec4<f32>(p, 1.0), c);
}
@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> { return vec4<f32>(in.color, 1.0); }
"#;

struct Chunk { voxels: [[[u8; CHUNK_SIZE]; CHUNK_SIZE]; CHUNK_SIZE] }
impl Chunk {
    fn generate() -> Self {
        let mut v = [[[0; CHUNK_SIZE]; CHUNK_SIZE]; CHUNK_SIZE];
        let mut rng = rand::thread_rng();
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                let h = (rng.gen::<f32>() * 5.0) as usize + 2;
                for y in 0..h { v[x][y][z] = 1; }
            }
        }
        Self { voxels: v }
    }
    fn mesh(&self, cx: f32, cz: f32) -> Vec<f32> {
        let mut v = Vec::new();
        for x in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    if self.voxels[x][y][z] != 0 {
                        let px = x as f32 + cx * CHUNK_SIZE as f32;
                        let py = y as f32;
                        let pz = z as f32 + cz * CHUNK_SIZE as f32;
                        v.extend_from_slice(&[px,py+1.0,pz, 0.5,0.5,0.5, px+1.0,py+1.0,pz, 0.5,0.5,0.5, px+1.0,py+1.0,pz+1.0, 0.5,0.5,0.5]);
                        v.extend_from_slice(&[px,py+1.0,pz, 0.5,0.5,0.5, px+1.0,py+1.0,pz+1.0, 0.5,0.5,0.5, px,py+1.0,pz+1.0, 0.5,0.5,0.5]);
                    }
                }
            }
        }
        v
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("[Atlas] Online.");
    let mut retina = Synapse::grow("renderer").await?;

    retina.fire(RenderCommand::RegisterPass {
        id: "voxel".into(),
        shader_source: VOXEL_SHADER.into(),
        topology: "TriangleList".into(),
    }).await?;

    for x in -2..2 {
        for z in -2..2 {
            let chunk = Chunk::generate();
            let verts = chunk.mesh(x as f32, z as f32);
            if verts.is_empty() { continue; }
            let id = format!("c_{}_{}", x, z);
            
            retina.fire(RenderCommand::UpdateResource { id: id.clone(), data: bytemuck::cast_slice(&verts).to_vec() }).await?;
            retina.fire(RenderCommand::SpawnEntity { id: id.clone(), pass_id: "voxel".into(), resource_id: id, vertex_count: verts.len() as u32 / 6 }).await?;
        }
    }

    loop { tokio::time::sleep(std::time::Duration::from_secs(10)).await; }
}