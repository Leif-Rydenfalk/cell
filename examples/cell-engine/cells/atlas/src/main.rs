use anyhow::Result;
use cell_sdk::cell_remote;
use cell_sdk::synapse::Synapse;

// Generates 'RendererService' client struct from the renderer's source code
cell_remote!(RendererService = "renderer");

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
    let mut retina = RendererService::connect().await?;
    println!("[Atlas] Connected.");

    // 1. Send Shader
    // Signature matches renderer: register_pass(id, shader_source, inputs, outputs, topology)
    retina.register_pass(
        "voxel".into(),
        SHADER.into(),
        vec![], // inputs
        vec![], // outputs
        "TriangleList".into()
    ).await?;

    // 2. Send Geometry
    let vertices: &[f32] = &[
        0.0, 0.5, 0.0,   1.0, 0.0, 0.0,
       -0.5, -0.5, 0.0,  0.0, 1.0, 0.0,
        0.5, -0.5, 0.0,  0.0, 0.0, 1.0,
    ];
    let data = bytemuck::cast_slice(vertices).to_vec();

    // update_resource(id, data)
    retina.update_resource("tri".into(), data).await?;
    
    // spawn_entity(id, pass_id, resource_id, vertex_count)
    retina.spawn_entity("ent1".into(), "voxel".into(), "tri".into(), 3).await?;

    println!("[Atlas] Job done.");
    loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
}