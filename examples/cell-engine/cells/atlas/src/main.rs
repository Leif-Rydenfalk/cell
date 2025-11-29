use anyhow::Result;
use cell_sdk::*;

// SYNCED CONTRACT: Must match Renderer exactly to ensure correct serialization layout
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
    SetCamera {
        position: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
    },
    GetInputState,
}

const SHADER: &str = r#"
struct VertexOutput { @builtin(position) pos: vec4<f32>, @location(0) color: vec3<f32>, };
@vertex fn vs_main(@location(0) p: vec3<f32>, @location(1) c: vec3<f32>) -> VertexOutput {
    return VertexOutput(vec4<f32>(p.x * 0.5, p.y * 0.5, p.z, 1.0), c);
}
@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> { return vec4<f32>(in.color, 1.0); }
"#;

#[tokio::main]
async fn main() -> Result<()> {
    println!("[Atlas] Connecting...");
    let mut retina = Synapse::grow("renderer").await?;

    // 1. Send Shader
    retina.fire(RenderCommand::RegisterPass {
        id: "voxel".into(),
        shader_source: SHADER.into(),
        topology: "TriangleList".into(),
    }).await?;

    // 2. Send Geometry
    let vertices: &[f32] = &[
        0.0, 0.5, 0.0,   1.0, 0.0, 0.0,
       -0.5, -0.5, 0.0,  0.0, 1.0, 0.0,
        0.5, -0.5, 0.0,  0.0, 0.0, 1.0,
    ];
    let data = bytemuck::cast_slice(vertices).to_vec();

    retina.fire(RenderCommand::UpdateResource { id: "tri".into(), data }).await?;
    retina.fire(RenderCommand::SpawnEntity { id: "ent1".into(), pass_id: "voxel".into(), resource_id: "tri".into(), vertex_count: 3 }).await?;

    println!("[Atlas] Job done.");
    loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
}