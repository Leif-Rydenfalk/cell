struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) v_idx: u32, @builtin(instance_index) i_idx: u32) -> VertexOut {
    let n = data[i_idx];
    
    // Skip inactive/empty neurons
    if (n.fatigue == 0.0) {
        var out: VertexOut;
        out.clip_pos = vec4<f32>(0.0);
        return out;
    }

    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0),  vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0)
    );
    let uv = corners[v_idx];
    
    // Match Logic Coordinate System
    let z = (f32(n.layer) - 3.0) * 2.0;
    let world_pos = vec3<f32>(n.pos.x * 10.0, n.pos.y * 10.0, z);
    
    // Billboard math
    let size = 0.03 + n.voltage * 0.1;
    
    // Simple billboard facing camera (assuming camera is generally looking down Z)
    // For true billboarding we need camera right/up vectors, but this suffices for now
    let billboard = world_pos + vec3<f32>(uv * size, 0.0);
    
    var out: VertexOut;
    out.clip_pos = camera.view_proj * vec4<f32>(billboard, 1.0);
    out.uv = uv;
    
    let l_norm = f32(n.layer) / 6.0;
    let base_col = vec3<f32>(l_norm, 0.5, 1.0 - l_norm);
    let active_col = vec3<f32>(1.0, 0.2, 0.1);
    
    out.color = vec4<f32>(mix(base_col, active_col, n.voltage), 1.0);
    
    return out;
}

@fragment
fn fs_main(@location(0) color: vec4<f32>, @location(1) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dist = length(uv);
    if (dist > 1.0) { discard; }
    return vec4<f32>(color.rgb, color.a * (1.0 - dist));
}