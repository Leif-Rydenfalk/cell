// --- STRUCTS ---

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

struct Neuron {
    semantic: vec4<u32>,  // 16 bytes - SDR Identity
    pos: vec2<f32>,       // 8 bytes
    voltage: f32,         // 4 bytes
    prediction: f32,      // 4 bytes
    precision_value: f32, // 4 bytes
    layer: u32,           // 4 bytes
    fatigue: f32,         // 4 bytes - BOOST factor for homeostasis
    boredom: f32,         // 4 bytes - padding/future use
};

struct SimParams {
    neuron_count: u32,
    time: f32,
    dt: f32,
    grid_dim: u32,
    train_mode: u32, 
    use_camera: u32, 
    pad0: f32,
    pad1: f32,
};

struct LineVertex {
    pos: vec4<f32>,
    color: vec4<f32>,
};

struct Camera {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    position: vec3<f32>,
    padding: u32,
    time: f32,
}

// --- BINDINGS ---

@group(0) @binding(0) var<storage, read_write> neurons: array<Neuron>;
@group(0) @binding(1) var<uniform> params: SimParams;
@group(0) @binding(2) var<storage, read_write> spatial_grid: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read_write> line_buffer: array<LineVertex>;
@group(0) @binding(4) var input_tex: texture_2d<f32>;
@group(0) @binding(5) var input_sampler: sampler;
@group(0) @binding(6) var output_tex: texture_storage_2d<rgba32float, write>;
@group(0) @binding(7) var prediction_tex: texture_storage_2d<rgba32float, write>;
@group(0) @binding(8) var prediction_tex_read: texture_2d<f32>;

@group(1) @binding(0) var<uniform> camera_uni: Camera;

// --- UTILITY ---

fn hash(seed: u32) -> u32 {
    var state = seed * 747796405u + 2891336453u;
    var word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn rand_f32(seed: u32) -> f32 {
    return f32(hash(seed)) / 4294967295.0;
}

fn rng_next(state: ptr<function, u32>) -> f32 {
    let old = *state;
    *state = old * 747796405u + 2891336453u;
    let word = ((*state >> ((*state >> 28u) + 4u)) ^ *state) * 277803737u;
    return f32((word >> 22u) ^ word) / 4294967295.0;
}

fn hamming_similarity(a: vec4<u32>, b: vec4<u32>) -> f32 {
    let diff = a ^ b;
    let bits = countOneBits(diff.x) + countOneBits(diff.y) + countOneBits(diff.z) + countOneBits(diff.w);
    return max(0.0, 1.0 - f32(bits) / 128.0); 
}

// --- COMPUTE SHADERS ---

@compute @workgroup_size(64)
fn cs_init_neurons(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= params.neuron_count) { return; }
    
    var n: Neuron;
    let seed = idx * 7123u;
    
    n.semantic = vec4<u32>(hash(seed), hash(seed+1u), hash(seed+2u), hash(seed+3u));
    n.voltage = 0.0;
    n.prediction = 0.0;
    n.fatigue = 1.0;
    n.boredom = 0.0;
    
    // EMERGENT LAYERING: Instead of assigning layers, assign Z-depth continuously
    // Neurons at Z=-1 will naturally become "input" layer
    // Neurons at Z=+1 will naturally become "prediction" layer
    // Middle neurons will self-organize based on connectivity
    
    let total = f32(params.neuron_count);
    let normalized_idx = f32(idx) / total;
    
    // Z-depth from -1.0 (input face) to +1.0 (output face)
    let z_base = normalized_idx * 2.0 - 1.0;
    
    // Add some jitter so layers aren't perfectly flat
    let z_jitter = (rand_f32(seed+7u) - 0.5) * 0.2;
    let z = clamp(z_base + z_jitter, -1.0, 1.0);
    
    // XY position is random across the 2D plane
    let x = rand_f32(seed+4u) * 2.0 - 1.0;
    let y = rand_f32(seed+5u) * 2.0 - 1.0;
    
    n.pos = vec2<f32>(x, y);
    
    // Store Z-depth in layer field (we'll reinterpret it)
    // Map -1.0...+1.0 to 0...6 for compatibility with grid system
    n.layer = u32(clamp((z + 1.0) * 3.5, 0.0, 6.0));
    
    n.precision_value = rand_f32(seed+6u);

    neurons[idx] = n;
}

@compute @workgroup_size(64)
fn cs_clear_grid(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    let layer_stride = params.grid_dim * params.grid_dim;
    if (idx >= layer_stride) { return; }

    for (var i = 0u; i < 8u; i++) {
        atomicStore(&spatial_grid[idx + i * layer_stride], 0xFFFFFFFFu);
    }
}

@compute @workgroup_size(64)
fn cs_populate_grid(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= params.neuron_count) { return; }
    
    let n = neurons[idx];
    let layer_stride = params.grid_dim * params.grid_dim;
    let layer_offset = clamp(n.layer, 0u, 7u) * layer_stride;
    
    let grid_dim = f32(params.grid_dim);
    let u = (n.pos.x + 1.0) * 0.5;
    let v = (n.pos.y + 1.0) * 0.5;
    let gx = u32(clamp(u * grid_dim, 0.0, grid_dim - 1.0));
    let gy = u32(clamp(v * grid_dim, 0.0, grid_dim - 1.0));
    let grid_idx = gx + gy * params.grid_dim;
    
    atomicStore(&spatial_grid[grid_idx + layer_offset], idx);
}

@compute @workgroup_size(64)
fn cs_update_neurons(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= params.neuron_count) { return; }
    
    var n = neurons[idx];
    
    // Decode Z-depth from layer field
    let my_depth = (f32(n.layer) / 3.5) - 1.0; // -1.0 to +1.0
    
    // --- BOTTOM LAYER: SENSORY INPUT ---
    // Neurons near Z=-1.0 are input neurons
    if (my_depth < -0.8) {
        let uv = (n.pos + 1.0) * 0.5;
        var reality = 0.0;
        if (params.use_camera == 1u) {
            reality = textureSampleLevel(input_tex, input_sampler, uv, 0.0).r;
        } else {
            reality = (rand_f32(idx + u32(params.time * 60.0)) - 0.5) * 0.1 + 0.5;
        }
        
        // Sample prediction from TOP layer (Z=+1.0)
        let pred = textureLoad(prediction_tex_read, vec2<i32>(uv * 512.0), 0).r;
        
        // Error = Surprise
        n.voltage = abs(reality - pred) * 2.0; // Amplify for visibility
        n.prediction = reality;
        
        neurons[idx] = n;
        return;
    }
    
    // --- TOP LAYER: PREDICTION OUTPUT ---
    // Neurons near Z=+1.0 are prediction neurons
    if (my_depth > 0.8) {
        // Top layer predicts by averaging similar neurons in middle layers
        var potential = 0.0;
        var max_sim = 0.0;
        var best_idx = 0xFFFFFFFFu;
        var rng = idx * 928374u + u32(params.time * 2.0);
        
        let grid_dim = f32(params.grid_dim);
        let center_u = (n.pos.x + 1.0) * 0.5 * grid_dim;
        let center_v = (n.pos.y + 1.0) * 0.5 * grid_dim;
        let stride = params.grid_dim * params.grid_dim;
        
        // Sample from middle layers (Z=-0.5 to +0.5)
        for (var i = 0u; i < 32u; i++) {
            let target_layer = u32(clamp(rng_next(&rng) * 3.5 + 1.5, 1.0, 5.0)); // Layers 1-5
            
            let r_angle = rng_next(&rng) * 6.28318;
            let r_dist = sqrt(rng_next(&rng)) * 8.0;
            
            let gx = u32(clamp(center_u + cos(r_angle) * r_dist, 0.0, grid_dim - 1.0));
            let gy = u32(clamp(center_v + sin(r_angle) * r_dist, 0.0, grid_dim - 1.0));
            
            let target_idx = atomicLoad(&spatial_grid[gx + gy * params.grid_dim + target_layer * stride]);
            
            if (target_idx != 0xFFFFFFFFu) {
                let src = neurons[target_idx];
                let sim = hamming_similarity(n.semantic, src.semantic);
                if (sim > 0.4) {
                    potential += src.voltage * sim;
                    
                    if (sim > max_sim) {
                        max_sim = sim;
                        best_idx = target_idx;
                    }
                }
            }
        }
        
        n.voltage = mix(n.voltage, potential / 32.0, 0.2);
        
        // TOP LAYER ALSO LEARNS
        if (params.train_mode == 1u && best_idx != 0xFFFFFFFFu) {
            let src = neurons[best_idx];
            let seed_learn = idx + u32(params.time * 500.0);
            let rnd = vec4<u32>(hash(seed_learn), hash(seed_learn+1u), hash(seed_learn+2u), hash(seed_learn+3u));
            let mask = rnd & vec4<u32>(0x0F0F0F0Fu);
            n.semantic = (n.semantic & ~mask) | (src.semantic & mask);
        }
        
        neurons[idx] = n;
        return;
    }

    // --- MIDDLE LAYERS: CORTICAL PROCESSING ---
    var potential = 0.0;
    var max_sim = 0.0;
    var best_source_idx = 0xFFFFFFFFu;
    
    var rng = idx * 928374u + u32(params.time * 2.0);
    
    let grid_dim = f32(params.grid_dim);
    let center_u = (n.pos.x + 1.0) * 0.5 * grid_dim;
    let center_v = (n.pos.y + 1.0) * 0.5 * grid_dim;
    let stride = params.grid_dim * params.grid_dim;
    
    // DEPTH-BASED CONNECTIVITY
    // Each neuron samples from a depth range based on its own depth
    let synapse_count = 64u;
    
    for (var i = 0u; i < synapse_count; i++) {
        let rnd_val = rng_next(&rng);
        
        // Sample from layers below (bottom-up) and same layer (lateral)
        // And occasionally from above (top-down feedback)
        var target_depth_layer: u32;
        
        if (rnd_val < 0.4) {
            // 40% bottom-up: sample from layers below
            target_depth_layer = max(0u, n.layer - 1u);
        } else if (rnd_val < 0.7) {
            // 30% lateral: sample from same layer
            target_depth_layer = n.layer;
        } else {
            // 30% top-down: sample from layers above
            target_depth_layer = min(6u, n.layer + 1u);
        }
        
        let reach = 8.0;
        let r_angle = rng_next(&rng) * 6.28318;
        let r_dist = sqrt(rng_next(&rng)) * reach;
        
        let gx = u32(clamp(center_u + cos(r_angle) * r_dist, 0.0, grid_dim - 1.0));
        let gy = u32(clamp(center_v + sin(r_angle) * r_dist, 0.0, grid_dim - 1.0));
        
        let target_idx = atomicLoad(&spatial_grid[gx + gy * params.grid_dim + target_depth_layer * stride]);
        
        if (target_idx != 0xFFFFFFFFu) {
            let src = neurons[target_idx];
            let sim = hamming_similarity(n.semantic, src.semantic);
            
            if (sim > 0.4) {
                potential += src.voltage * sim;
                
                if (sim > max_sim) {
                    max_sim = sim;
                    best_source_idx = target_idx;
                }
            }
        }
    }
    
    potential *= n.fatigue;
    
    // --- LATERAL INHIBITION ---
    var stronger_neighbors = 0u;
    let check_count = 32u;
    
    for (var k = 0u; k < check_count; k++) {
        let r_angle = rng_next(&rng) * 6.28;
        let r_dist = rng_next(&rng) * 25.0;
        
        let gx = u32(clamp(center_u + cos(r_angle) * r_dist, 0.0, grid_dim - 1.0));
        let gy = u32(clamp(center_v + sin(r_angle) * r_dist, 0.0, grid_dim - 1.0));
        
        let n_idx = atomicLoad(&spatial_grid[gx + gy * params.grid_dim + n.layer * stride]);
        
        if (n_idx != 0xFFFFFFFFu && n_idx != idx) {
            let neighbor = neurons[n_idx];
            if (neighbor.voltage > potential) {
                stronger_neighbors++;
            }
        }
    }
    
    var is_active = false;
    if (stronger_neighbors < 4u && potential > 0.1) {
        n.voltage = mix(n.voltage, 1.0, 0.3);
        is_active = true;
    } else {
        n.voltage = mix(n.voltage, 0.0, 0.5);
    }
    
    // HOMEOSTASIS
    if (is_active) {
        n.fatigue = max(0.5, n.fatigue - 0.05);
    } else {
        n.fatigue = min(10.0, n.fatigue + 0.001);
    }
    
    // LEARNING (Much more aggressive)
    if (is_active && params.train_mode == 1u && best_source_idx != 0xFFFFFFFFu) {
        let src = neurons[best_source_idx];
        let seed_learn = idx + u32(params.time * 500.0);
        
        // Learn on EVERY activation, not randomly
        // Update 4 bits instead of 1
        let rnd = vec4<u32>(hash(seed_learn), hash(seed_learn+1u), hash(seed_learn+2u), hash(seed_learn+3u));
        let mask = rnd & vec4<u32>(0x0F0F0F0Fu); // 4 bits per byte instead of 1
        n.semantic = (n.semantic & ~mask) | (src.semantic & mask);
    }
    
    // BURSTING (More aggressive exploration)
    if (n.fatigue > 3.0 && params.train_mode == 1u) {  // Lower threshold from 5.0 to 3.0
        let seed_mut = idx + u32(params.time * 77.0);
        let mask = vec4<u32>(0x0F0F0F0Fu);  // Mutate 4 bits instead of 1
        let new_bit = vec4<u32>(hash(seed_mut), hash(seed_mut+1u), hash(seed_mut+2u), hash(seed_mut+3u));
        n.semantic = (n.semantic & ~mask) | (new_bit & mask);
    }

    neurons[idx] = n;
}

@compute @workgroup_size(64)
fn cs_generate_lines(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= params.neuron_count) { return; }
    
    let n = neurons[idx];
    
    if (n.voltage < 0.1 || n.layer == 0u) {
        line_buffer[idx*2].pos = vec4<f32>(0.0);
        line_buffer[idx*2+1].pos = vec4<f32>(0.0);
        return;
    }
    
    var rng = idx * 928374u + u32(params.time * 2.0);
    var best_src_pos = vec3<f32>(0.0);
    var found = false;
    var max_sim = 0.0;
    
    let grid_dim = f32(params.grid_dim);
    let stride = params.grid_dim * params.grid_dim;
    let center_u = (n.pos.x + 1.0) * 0.5 * grid_dim;
    let center_v = (n.pos.y + 1.0) * 0.5 * grid_dim;
    
    // Sample to find best connection to visualize
    for(var i=0u; i<16u; i++) {
        let rnd_val = rng_next(&rng);
        var target_layer = n.layer;
        
        if (n.layer == 1u) {
            target_layer = select(1u, 2u, rnd_val > 0.5);
        } else if (n.layer == 2u) {
            target_layer = select(1u, 4u, rnd_val > 0.5);
        } else if (n.layer == 4u) {
            target_layer = select(6u, 0u, rnd_val > 0.3);
        }
        
        let reach = 8.0;
        let r_angle = rng_next(&rng) * 6.28;
        let r_dist = sqrt(rng_next(&rng)) * reach;
        
        let gx = u32(clamp(center_u + cos(r_angle) * r_dist, 0.0, grid_dim - 1.0));
        let gy = u32(clamp(center_v + sin(r_angle) * r_dist, 0.0, grid_dim - 1.0));
        
        let t_idx = atomicLoad(&spatial_grid[gx + gy * params.grid_dim + target_layer * stride]);
        
        if (t_idx != 0xFFFFFFFFu) {
            let src = neurons[t_idx];
            let sim = hamming_similarity(n.semantic, src.semantic);
            
            if (sim > max_sim && sim > 0.5) {
                max_sim = sim;
                let z_src = (f32(src.layer) - 3.0) * 0.5;
                best_src_pos = vec3<f32>(src.pos, z_src) * 3.0;
                found = true;
            }
        }
    }
    
    if (found) {
        let z_curr = (f32(n.layer) - 3.0) * 0.5;
        let p1 = vec3<f32>(n.pos, z_curr) * 3.0;
        
        var col = vec4<f32>(1.0);
        if (n.layer == 1u) {
            col = vec4<f32>(1.0, 0.0, 1.0, 0.4); // Purple: L1 feedback
        } else if (n.layer == 2u) {
            col = vec4<f32>(1.0, 1.0, 0.0, 0.3); // Yellow: L2 processing
        } else if (n.layer == 4u) {
            col = vec4<f32>(0.0, 1.0, 0.0, 0.4); // Green: L4 input
        } else {
            col = vec4<f32>(0.2, 0.5, 1.0, 0.2); // Blue: other
        }
        
        line_buffer[idx*2].pos = vec4<f32>(p1, 1.0);
        line_buffer[idx*2].color = col;
        line_buffer[idx*2+1].pos = vec4<f32>(best_src_pos, 1.0);
        line_buffer[idx*2+1].color = col;
    } else {
        line_buffer[idx*2].pos = vec4<f32>(0.0);
        line_buffer[idx*2+1].pos = vec4<f32>(0.0);
    }
}

@compute @workgroup_size(8, 8)
fn cs_render_dream(@builtin(global_invocation_id) id: vec3<u32>) {
    let dim = vec2<u32>(512, 512);
    if (id.x >= dim.x || id.y >= dim.y) { return; }
    
    let uv = vec2<f32>(f32(id.x) / 512.0, f32(id.y) / 512.0);
    let pos = uv * 2.0 - 1.0;
    
    var val = 0.0;
    var w_sum = 0.0;
    
    let grid_dim = f32(params.grid_dim);
    let center_u = uv.x * grid_dim;
    let center_v = uv.y * grid_dim;
    let layer_stride = params.grid_dim * params.grid_dim;
    
    // Sample from TOP layer (L6, depth > 0.8)
    // These are layers 5 and 6 in our depth mapping
    for (var layer_offset = 5u; layer_offset <= 6u; layer_offset++) {
        let grid_offset = layer_offset * layer_stride;
        
        for (var dy = -2; dy <= 2; dy++) {
            for (var dx = -2; dx <= 2; dx++) {
                let gx = u32(clamp(center_u + f32(dx), 0.0, grid_dim - 1.0));
                let gy = u32(clamp(center_v + f32(dy), 0.0, grid_dim - 1.0));
                let idx = atomicLoad(&spatial_grid[gx + gy * params.grid_dim + grid_offset]);
                 
                if (idx != 0xFFFFFFFFu) {
                    let n = neurons[idx];
                    let n_depth = (f32(n.layer) / 3.5) - 1.0;
                    
                    // Only use top layer neurons
                    if (n_depth > 0.8) {
                        let d = distance(pos, n.pos);
                        let w = exp(-d * 50.0);
                        val += n.voltage * w;
                        w_sum += w;
                    }
                }
            }
        }
    }
    
    // If no neurons found (early in training), default to gray
    let final_color = select(0.5, val / w_sum, w_sum > 0.001);
    textureStore(prediction_tex, vec2<i32>(id.xy), vec4<f32>(final_color, final_color, final_color, 1.0));
}

@compute @workgroup_size(8, 8)
fn cs_render_error(@builtin(global_invocation_id) id: vec3<u32>) {
    let dim = vec2<u32>(512, 512);
    if (id.x >= dim.x || id.y >= dim.y) { return; }
    
    let uv = vec2<f32>(f32(id.x) / 512.0, f32(id.y) / 512.0);
    let pos = uv * 2.0 - 1.0;
    
    var error_accum = 0.0;
    var w_sum = 0.0;
    
    let grid_dim = f32(params.grid_dim);
    let center_u = uv.x * grid_dim;
    let center_v = uv.y * grid_dim;
    let layer_stride = params.grid_dim * params.grid_dim;
    
    // Sample from BOTTOM layer (L0, depth < -0.8)
    // This is layer 0 in our depth mapping
    let grid_offset = 0u * layer_stride;
    
    for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
            let gx = u32(clamp(center_u + f32(dx), 0.0, grid_dim - 1.0));
            let gy = u32(clamp(center_v + f32(dy), 0.0, grid_dim - 1.0));
            let idx = atomicLoad(&spatial_grid[gx + gy * params.grid_dim + grid_offset]);
            
            if (idx != 0xFFFFFFFFu) {
                let n = neurons[idx];
                let n_depth = (f32(n.layer) / 3.5) - 1.0;
                
                // Only use bottom layer neurons
                if (n_depth < -0.8) {
                    let d = distance(pos, n.pos);
                    let w = exp(-d * 100.0);
                    
                    // n.voltage in L0 contains abs(reality - prediction)
                    error_accum += n.voltage * w;
                    w_sum += w;
                }
            }
        }
    }
    
    let error_val = select(0.0, error_accum / w_sum, w_sum > 0.0001);
    
    // Visualize error magnitude as red intensity
    // Perfect prediction = black, high error = bright red
    let col = vec4<f32>(error_val * 2.0, 0.0, 0.0, 1.0);
    
    textureStore(output_tex, vec2<i32>(id.xy), col);
}

// --- RENDERING SHADERS ---

@vertex
fn vs_main(@location(0) vertex_pos: vec3<f32>, @builtin(instance_index) idx: u32) -> VertexOutput {
    var out: VertexOutput;
    let n = neurons[idx];
    
    // Decode actual Z depth
    let z_depth = (f32(n.layer) / 3.5 - 1.0) * 1.5; // Scale for visibility
    
    let world_pos = vec3<f32>(n.pos, z_depth) * 3.0 + vertex_pos * 0.015;
    out.clip_position = camera_uni.view_proj * vec4<f32>(world_pos, 1.0);
    
    var col = vec3<f32>(0.0);
    let display_val = n.voltage;
    
    // Color by depth gradient
    let depth_norm = (z_depth + 1.5) / 3.0; // 0 to 1
    
    if (depth_norm < 0.15) {
        // Bottom (input): Red
        col = vec3<f32>(display_val, 0.0, 0.0);
    } else if (depth_norm > 0.85) {
        // Top (prediction): Blue
        col = vec3<f32>(0.0, 0.0, display_val);
    } else {
        // Middle (cortex): White/cyan with boost glow
        let boost_glow = (n.fatigue - 1.0) * 0.1;
        col = vec3<f32>(display_val * 0.5, display_val, display_val * 0.5 + boost_glow);
    }
    
    out.color = vec4<f32>(col, 1.0);
    return out;
}

@fragment
fn fs_main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}

@vertex
fn vs_lines(@location(0) pos: vec4<f32>, @location(1) col: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = camera_uni.view_proj * vec4<f32>(pos.xyz, 1.0);
    out.color = col;
    return out;
}

@fragment
fn fs_lines(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}