// src/shaders/voxel_tracer.wgsl

// ==========================================
// 1. CONFIG & STRUCTS
// ==========================================

const GRID_SIZE: u32 = 128u;
const MAX_DIST: f32 = 250.0;

struct Global {
    time: f32,
    dt: f32,
    frame: u32,
    _pad1: u32,
    mouse: vec4<f32>, // x, y = ndc, z, w = buttons
    screen: vec2<f32>,
}

// Output for the Vertex Shader
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// ==========================================
// 2. BINDINGS
// ==========================================

// Global Uniforms (Set 1)
@group(1) @binding(0) var<uniform> global: Global;

// Resources (Set 0)
// Binding 0: The Voxel Grid (u32 where 0 = empty, >0 = color data)
@group(0) @binding(0) var<storage, read_write> voxels: array<u32>;

// Binding 1: The Output Texture (Compute writes here)
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

// Binding 1 (in Render Pass): The Output Texture (Fragment reads here)
@group(0) @binding(2) var display_tex: texture_2d<f32>;
@group(0) @binding(3) var display_sampler: sampler;

// ==========================================
// 3. HELPER FUNCTIONS
// ==========================================

fn get_voxel_index(p: vec3<i32>) -> u32 {
    if (p.x < 0 || p.x >= i32(GRID_SIZE) || 
        p.y < 0 || p.y >= i32(GRID_SIZE) || 
        p.z < 0 || p.z >= i32(GRID_SIZE)) {
        return 0xFFFFFFFFu; // Out of bounds
    }
    return u32(p.x) + u32(p.y) * GRID_SIZE + u32(p.z) * GRID_SIZE * GRID_SIZE;
}

fn unpack_color(c: u32) -> vec3<f32> {
    let r = f32((c >> 16u) & 0xFFu) / 255.0;
    let g = f32((c >> 8u) & 0xFFu) / 255.0;
    let b = f32(c & 0xFFu) / 255.0;
    return vec3<f32>(r, g, b);
}

fn pack_color(rgb: vec3<f32>) -> u32 {
    let r = u32(clamp(rgb.x, 0.0, 1.0) * 255.0);
    let g = u32(clamp(rgb.y, 0.0, 1.0) * 255.0);
    let b = u32(clamp(rgb.z, 0.0, 1.0) * 255.0);
    return (r << 16u) | (g << 8u) | b;
}

// SDF for world generation
fn sd_sphere(p: vec3<f32>, s: f32) -> f32 {
    return length(p) - s;
}

fn map_range(v: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    return out_min + (out_max - out_min) * (v - in_min) / (in_max - in_min);
}

// ==========================================
// 4. COMPUTE: INIT / ANIMATION
// ==========================================

@compute @workgroup_size(8, 8, 8)
fn cs_init(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= GRID_SIZE || id.y >= GRID_SIZE || id.z >= GRID_SIZE) { return; }
    
    let idx = get_voxel_index(vec3<i32>(id));
    
    // Normalize coordinates -1.0 to 1.0
    let center = vec3<f32>(GRID_SIZE) * 0.5;
    let pos = (vec3<f32>(id) - center) / center;
    
    // Animate shape
    let t = global.time * 0.5;
    
    // 1. Moving Floor
    var val = pos.y + 0.8 + sin(pos.x * 5.0 + t) * 0.1 * cos(pos.z * 5.0 + t * 0.5);
    
    // 2. Floating Sphere
    let sphere_pos = pos - vec3<f32>(sin(t)*0.5, 0.2, cos(t)*0.5);
    let sphere = sd_sphere(sphere_pos, 0.3);
    
    // Logic to set voxel
    var color = 0u;
    
    if (sphere < 0.0) {
        // Sphere Color (Gradient)
        color = pack_color(vec3<f32>(0.9, 0.4 + abs(pos.y), 0.2));
    } else if (val < 0.0) {
        // Floor Color (Checkers)
        let check = (id.x + id.z) % 2u;
        if (check == 0u) {
            color = pack_color(vec3<f32>(0.2, 0.2, 0.25));
        } else {
            color = pack_color(vec3<f32>(0.3, 0.3, 0.35));
        }
    }
    
    voxels[idx] = color;
}

// ==========================================
// 5. COMPUTE: RAYTRACING
// ==========================================

@compute @workgroup_size(16, 16)
fn cs_trace(@builtin(global_invocation_id) id: vec3<u32>) {
    let screen_dims = textureDimensions(output_tex);
    if (id.x >= screen_dims.x || id.y >= screen_dims.y) { return; }

    let uv = (vec2<f32>(id.xy) / vec2<f32>(screen_dims)) * 2.0 - 1.0;
    let aspect = f32(screen_dims.x) / f32(screen_dims.y);

    // Camera setup (Orbit)
    let rot_speed = 0.5;
    let cam_dist = 160.0;
    
    // Mouse interaction for rotation
    let mx = global.mouse.x * 2.0; 
    let my = global.mouse.y * 1.5;

    let ro = vec3<f32>(
        sin(global.time * 0.1 + mx) * cam_dist,
        map_range(my, -1.0, 1.0, 20.0, 100.0),
        cos(global.time * 0.1 + mx) * cam_dist
    );
    
    // RENAMED from 'target' to 'cam_target' to avoid keyword conflict
    let cam_target = vec3<f32>(GRID_SIZE) * 0.5;
    
    let fwd = normalize(cam_target - ro);
    let right = normalize(cross(fwd, vec3<f32>(0.0, 1.0, 0.0)));
    let up = cross(right, fwd);
    
    let rd = normalize(fwd + right * uv.x * aspect + up * uv.y); 

    // DDA Setup
    var map_pos = vec3<i32>(floor(ro));
    
    // Delta distance (distance ray travels to cross one grid unit in X/Y/Z)
    // abs(1.0 / rd) is the simplification
    let delta_dist = vec3<f32>(
        abs(1.0 / rd.x),
        abs(1.0 / rd.y),
        abs(1.0 / rd.z)
    );
    
    let step_dir = vec3<i32>(sign(rd));
    
    // Initial side distance
    var side_dist = vec3<f32>(0.0);
    
    if (rd.x < 0.0) { side_dist.x = (ro.x - f32(map_pos.x)) * delta_dist.x; }
    else            { side_dist.x = (f32(map_pos.x) + 1.0 - ro.x) * delta_dist.x; }
    
    if (rd.y < 0.0) { side_dist.y = (ro.y - f32(map_pos.y)) * delta_dist.y; }
    else            { side_dist.y = (f32(map_pos.y) + 1.0 - ro.y) * delta_dist.y; }
    
    if (rd.z < 0.0) { side_dist.z = (ro.z - f32(map_pos.z)) * delta_dist.z; }
    else            { side_dist.z = (f32(map_pos.z) + 1.0 - ro.z) * delta_dist.z; }

    var mask = vec3<f32>(0.0);
    var hit = false;
    var color = vec3<f32>(0.05, 0.05, 0.08); // Background color
    var steps = 0;

    // Traversal Loop
    loop {
        if (steps > 300) { break; } // Safety break
        
        let idx = get_voxel_index(map_pos);
        if (idx != 0xFFFFFFFFu) {
            let voxel_data = voxels[idx];
            if (voxel_data != 0u) {
                hit = true;
                let albedo = unpack_color(voxel_data);
                
                // Simple lighting
                // mask contains 1.0 in the axis we hit
                var normal = vec3<f32>(0.0);
                if (mask.x > 0.0) { normal.x = -f32(step_dir.x); }
                else if (mask.y > 0.0) { normal.y = -f32(step_dir.y); }
                else { normal.z = -f32(step_dir.z); }
                
                let light_dir = normalize(vec3<f32>(0.5, 1.0, -0.5));
                let diff = max(dot(normal, light_dir), 0.0);
                let ambient = 0.2;
                
                color = albedo * (diff + ambient);
                
                // Fog
                let dist = length(vec3<f32>(map_pos) - ro);
                color = mix(color, vec3<f32>(0.05, 0.05, 0.08), clamp(dist / 200.0, 0.0, 1.0));
                break;
            }
        }

        // Stepping logic
        if (side_dist.x < side_dist.y) {
            if (side_dist.x < side_dist.z) {
                side_dist.x += delta_dist.x;
                map_pos.x += step_dir.x;
                mask = vec3<f32>(1.0, 0.0, 0.0);
            } else {
                side_dist.z += delta_dist.z;
                map_pos.z += step_dir.z;
                mask = vec3<f32>(0.0, 0.0, 1.0);
            }
        } else {
            if (side_dist.y < side_dist.z) {
                side_dist.y += delta_dist.y;
                map_pos.y += step_dir.y;
                mask = vec3<f32>(0.0, 1.0, 0.0);
            } else {
                side_dist.z += delta_dist.z;
                map_pos.z += step_dir.z;
                mask = vec3<f32>(0.0, 0.0, 1.0);
            }
        }
        
        // Out of bounds check optimization
        if (map_pos.y < -10 || map_pos.y > i32(GRID_SIZE) + 10) { break; }
        
        steps++;
    }

    textureStore(output_tex, id.xy, vec4<f32>(color, 1.0));
}

// ==========================================
// 6. RENDER: FULLSCREEN PASS
// ==========================================

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    // Fullscreen triangle
    var uv = vec2<f32>(f32((in_vertex_index << 1u) & 2u), f32(in_vertex_index & 2u));
    var out: VertexOutput;
    out.clip_position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    // Flip Y for texture sampling in fragment shader
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y); 
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(display_tex, display_sampler, in.uv);
}