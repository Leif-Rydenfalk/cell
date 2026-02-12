// struct LineVertex is automatically injected by the framework from brain_logic.wgsl
// Do not redefine it here.

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) v_idx: u32) -> VertexOut {
    // 'data' is array<LineVertex> provided by the framework
    let v = data[v_idx];
    
    var out: VertexOut;
    out.clip_pos = camera.view_proj * v.pos;
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}