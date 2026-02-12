// COMPLETE FIXED VERSION - src/shaders/brain_logic.wgsl
// This version addresses all ping-pong, initialization, and grid population issues

// --- STRUCTS (Auto-injected by framework) ---
struct Neuron {
    semantic: vec4<u32>,
    pos: vec2<f32>,
    voltage: f32,
    prediction: f32,
    precision_val: f32,
    layer: u32,
    fatigue: f32,
    pad: f32,
};

struct LineVertex {
    pos: vec4<f32>,
    color: vec4<f32>,
};

struct Params {
    learning_rate: f32,
    decay: f32, 
    sensitivity: f32,
    noise_lvl: f32,
};

// --- BINDINGS (Auto-injected by framework based on config) ---
// @group(0) @binding(0) var<uniform> params: Params;
// @group(0) @binding(1) var<storage, read> neurons: array<Neuron>;  // READ buffer
// @group(0) @binding(2) var<storage, read_write> neurons_out: array<Neuron>;  // WRITE buffer
// @group(0) @binding(3) var<storage, read_write> spatialgrid: array<atomic<u32>>;
// @group(0) @binding(4+) textures...
// @group(1) @binding(0) var<uniform> global: GlobalParams;

// --- UTILS (From common.wgsl) ---
fn hash(x: u32) -> u32 {
    var state = x * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
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

fn hamming(a: vec4<u32>, b: vec4<u32>) -> f32 {
    let diff = a ^ b;
    let bits = countOneBits(diff.x) + countOneBits(diff.y) + 
               countOneBits(diff.z) + countOneBits(diff.w);
    return max(0.0, 1.0 - f32(bits) / 128.0); 
}

// ============================================================================
// GRID OPERATIONS
// ============================================================================

@compute @workgroup_size(64)
fn cs_clear_grid(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= 262144u) { return; }  // 512*512
    
    // Clear all 8 layers
    for (var layer = 0u; layer < 8u; layer++) {
        atomicStore(&spatialgrid[idx + layer * 262144u], 0xFFFFFFFFu);
    }
}

@compute @workgroup_size(64)
fn cs_populate_grid(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= arrayLength(&neurons)) { return; }
    
    // CRITICAL FIX: Read from 'neurons' (current READ buffer)
    let n = neurons[idx];
    
    // Skip uninitialized neurons (fatigue == 0 means never initialized)
    if (n.fatigue == 0.0) { return; }

    let grid_dim = 512u;
    let layer_stride = 262144u;
    let layer_offset = clamp(n.layer, 0u, 7u) * layer_stride;
    
    let u = (n.pos.x + 1.0) * 0.5;
    let v = (n.pos.y + 1.0) * 0.5;
    let gx = u32(clamp(u * 512.0, 0.0, 511.0));
    let gy = u32(clamp(v * 512.0, 0.0, 511.0));
    
    let grid_idx = gx + gy * grid_dim + layer_offset;
    atomicStore(&spatialgrid[grid_idx], idx);
}

// ============================================================================
// MAIN UPDATE KERNEL
// ============================================================================

@compute @workgroup_size(64)
fn cs_update_neurons(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= arrayLength(&neurons_out)) { return; }
    
    // CRITICAL: Read from 'neurons' (previous state)
    let n_old = neurons[idx];
    var n_new = n_old;
    
    var rng = idx * 928374u + u32(global.time * 100.0);
    
    // ========================================================================
    // INITIALIZATION (First frame only)
    // ========================================================================
    if (n_old.fatigue == 0.0) {
        let seed = idx * 7123u;
        
        // Initialize semantic identity
        n_new.semantic = vec4<u32>(
            hash(seed), 
            hash(seed+1u), 
            hash(seed+2u), 
            hash(seed+3u)
        );
        
        // Initialize state
        n_new.voltage = 0.0;
        n_new.prediction = 0.0;
        n_new.fatigue = 1.0;  // CRITICAL: Mark as initialized
        
        // Random 2D position
        n_new.pos = vec2<f32>(
            rand_f32(seed+4u) * 2.0 - 1.0,
            rand_f32(seed+5u) * 2.0 - 1.0
        );
        
        // Assign layer based on index (creates depth stratification)
        let total = f32(arrayLength(&neurons_out));
        let normalized_idx = f32(idx) / total;
        let z = clamp(
            (normalized_idx * 2.0 - 1.0) + (rand_f32(seed+6u) - 0.5) * 0.2,
            -1.0, 
            1.0
        );
        n_new.layer = u32(clamp((z + 1.0) * 3.5, 0.0, 6.0));
        
        n_new.precision_val = rand_f32(seed+7u);
        
        // Write and return immediately
        neurons_out[idx] = n_new;
        return;
    }

    // ========================================================================
    // COMPUTE DEPTH (for layer-based behavior)
    // ========================================================================
    let my_depth = (f32(n_old.layer) / 3.5) - 1.0;  // Maps to -1.0 to +1.0
    
    // ========================================================================
    // SENSORY LAYER (Bottom layer, depth < -0.8)
    // ========================================================================
    if (my_depth < -0.8) {
        let uv = (n_old.pos + 1.0) * 0.5;
        let dims = vec2<f32>(textureDimensions(camera));
        let coord = vec2<i32>(uv * dims);
        
        // Sample camera input
        let reality = textureLoad(camera, coord, 0).r;
        
        // Compute prediction error
        let error = abs(reality - n_old.prediction);
        n_new.voltage = error * 2.0;  // Amplify for visibility
        n_new.prediction = reality;  // Store for next frame
        
        // Write output
        neurons_out[idx] = n_new;
        return;
    }

    // ========================================================================
    // PREDICTION LAYER (Top layer, depth > 0.8)
    // ========================================================================
    if (my_depth > 0.8) {
        var potential = 0.0;
        var weight_sum = 0.0;
        var max_sim = 0.0;
        var best_src_idx = 0xFFFFFFFFu;
        
        let center_u = (n_old.pos.x + 1.0) * 0.5 * 512.0;
        let center_v = (n_old.pos.y + 1.0) * 0.5 * 512.0;
        
        // Sample from middle layers (layers 2-5)
        for (var i = 0u; i < 32u; i++) {
            let target_layer = u32(clamp(rng_next(&rng) * 4.0 + 2.0, 2.0, 5.0));
            
            let angle = rng_next(&rng) * 6.28318;
            let dist = sqrt(rng_next(&rng)) * 12.0;
            
            let gx = u32(clamp(center_u + cos(angle) * dist, 0.0, 511.0));
            let gy = u32(clamp(center_v + sin(angle) * dist, 0.0, 511.0));
            
            let neighbor_idx = atomicLoad(&spatialgrid[gx + gy * 512u + target_layer * 262144u]);
            
            if (neighbor_idx != 0xFFFFFFFFu && neighbor_idx < arrayLength(&neurons)) {
                // Read from 'neurons' array (previous state)
                let src = neurons[neighbor_idx];
                let sim = hamming(n_old.semantic, src.semantic);
                
                if (sim > 0.4) {
                    potential += src.voltage * sim;
                    weight_sum += sim;
                    
                    if (sim > max_sim) {
                        max_sim = sim;
                        best_src_idx = neighbor_idx;
                    }
                }
            }
        }
        
        // Update voltage as weighted average
        if (weight_sum > 0.0) {
            n_new.voltage = mix(n_old.voltage, potential / weight_sum, 0.2);
        }
        
        // Learning: adapt semantic to best match
        if (best_src_idx != 0xFFFFFFFFu) {
            let src = neurons[best_src_idx];
            let seed_learn = idx + u32(global.time * 500.0);
            let rnd = vec4<u32>(
                hash(seed_learn), 
                hash(seed_learn+1u), 
                hash(seed_learn+2u), 
                hash(seed_learn+3u)
            );
            let mask = vec4<u32>(0x0F0F0F0Fu);
            n_new.semantic = (n_old.semantic & ~mask) | (src.semantic & mask);
        }
        
        neurons_out[idx] = n_new;
        return;
    }

    // ========================================================================
    // CORTICAL LAYER (Middle layers, -0.8 <= depth <= 0.8)
    // ========================================================================
    var potential = 0.0;
    var max_sim = 0.0;
    var best_src_idx = 0xFFFFFFFFu;
    
    let center_u = (n_old.pos.x + 1.0) * 0.5 * 512.0;
    let center_v = (n_old.pos.y + 1.0) * 0.5 * 512.0;
    
    // Sample 32 neighbors with depth-based connectivity
    for (var i = 0u; i < 32u; i++) {
        let rnd_val = rng_next(&rng);
        
        // Determine target layer (bottom-up, lateral, top-down)
        var target_layer = n_old.layer;
        if (rnd_val < 0.4) {
            target_layer = max(0u, n_old.layer - 1u);  // Bottom-up
        } else if (rnd_val > 0.7) {
            target_layer = min(6u, n_old.layer + 1u);  // Top-down
        }
        // else: lateral (same layer)
        
        let angle = rng_next(&rng) * 6.28318;
        let dist = sqrt(rng_next(&rng)) * 12.0;
        
        let gx = u32(clamp(center_u + cos(angle) * dist, 0.0, 511.0));
        let gy = u32(clamp(center_v + sin(angle) * dist, 0.0, 511.0));
        
        let neighbor_idx = atomicLoad(&spatialgrid[gx + gy * 512u + target_layer * 262144u]);
        
        if (neighbor_idx != 0xFFFFFFFFu && neighbor_idx < arrayLength(&neurons)) {
            // CRITICAL: Read from 'neurons' (previous frame state)
            let src = neurons[neighbor_idx];
            let sim = hamming(n_old.semantic, src.semantic);
            
            if (sim > 0.3) {
                potential += src.voltage * sim;
                
                if (sim > max_sim) {
                    max_sim = sim;
                    best_src_idx = neighbor_idx;
                }
            }
        }
    }
    
    // Apply homeostatic fatigue
    potential *= n_old.fatigue;
    
    // Activation threshold
    if (potential > 0.15) {
        n_new.voltage = mix(n_old.voltage, 1.0, 0.3);
        n_new.fatigue = max(0.5, n_old.fatigue - 0.05);
        
        // Hebbian learning
        if (best_src_idx != 0xFFFFFFFFu) {
            let src = neurons[best_src_idx];
            let seed_learn = idx + u32(global.time * 500.0);
            let rnd = vec4<u32>(
                hash(seed_learn), 
                hash(seed_learn+1u), 
                hash(seed_learn+2u), 
                hash(seed_learn+3u)
            );
            let mask = vec4<u32>(0x03030303u);
            n_new.semantic = (n_new.semantic & ~mask) | (src.semantic & mask);
        }
    } else {
        n_new.voltage = mix(n_old.voltage, 0.0, 0.1);
        n_new.fatigue = min(2.0, n_old.fatigue + 0.005);
    }
    
    // Write new state
    neurons_out[idx] = n_new;
}

// ============================================================================
// LINE GENERATION (For visualization)
// ============================================================================

@compute @workgroup_size(64)
fn cs_generate_lines(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    let n_idx = idx / 2u;
    let is_end = (idx % 2u) == 1u;

    if (n_idx >= arrayLength(&neurons)) { 
        // Write null position
        lines[idx].pos = vec4<f32>(0.0);
        return; 
    }
    
    // Read neuron state
    let n = neurons[n_idx];
    
    // Skip inactive neurons
    if (n.voltage < 0.2 || n.layer == 0u || n.fatigue == 0.0) { 
        lines[idx].pos = vec4<f32>(0.0);
        return; 
    }

    let z_curr = (f32(n.layer) - 3.0) * 2.0;
    let p_start = vec3<f32>(n.pos.x * 10.0, n.pos.y * 10.0, z_curr);

    // Determine color by layer
    var col = vec4<f32>(0.2, 0.5, 1.0, 0.3);
    if (n.layer == 1u) { col = vec4<f32>(1.0, 0.0, 1.0, 0.5); }
    else if (n.layer == 2u) { col = vec4<f32>(1.0, 1.0, 0.0, 0.5); }
    else if (n.layer == 4u) { col = vec4<f32>(0.0, 1.0, 0.0, 0.5); }

    // Find best connection
    var rng = n_idx * 999u + u32(global.time * 2.0);
    var found = false;
    var best_pos = vec3<f32>(0.0);
    var max_sim = 0.0;

    let center_u = (n.pos.x + 1.0) * 0.5 * 512.0;
    let center_v = (n.pos.y + 1.0) * 0.5 * 512.0;

    for (var i = 0u; i < 8u; i++) {
        let angle = rng_next(&rng) * 6.28318;
        let dist = sqrt(rng_next(&rng)) * 12.0;
        let gx = u32(clamp(center_u + cos(angle) * dist, 0.0, 511.0));
        let gy = u32(clamp(center_v + sin(angle) * dist, 0.0, 511.0));
        let target_layer = max(0u, n.layer - 1u);
        
        let t_idx = atomicLoad(&spatialgrid[gx + gy * 512u + target_layer * 262144u]);
        
        if (t_idx != 0xFFFFFFFFu && t_idx < arrayLength(&neurons)) {
            let src = neurons[t_idx];
            let sim = hamming(n.semantic, src.semantic);
            
            if (sim > max_sim && sim > 0.4) {
                max_sim = sim;
                let z_src = (f32(src.layer) - 3.0) * 2.0;
                best_pos = vec3<f32>(src.pos.x * 10.0, src.pos.y * 10.0, z_src);
                found = true;
            }
        }
    }

    if (found) {
        if (is_end) {
            lines[idx].pos = vec4<f32>(best_pos, 1.0);
        } else {
            lines[idx].pos = vec4<f32>(p_start, 1.0);
        }
        lines[idx].color = col;
    } else {
        lines[idx].pos = vec4<f32>(0.0);
    }
}

// ============================================================================
// TEXTURE RENDERING
// ============================================================================

@compute @workgroup_size(8, 8)
fn cs_render_dream(@builtin(global_invocation_id) id: vec3<u32>) {
    let coord = vec2<i32>(id.xy);
    if (id.x >= 512u || id.y >= 512u) { return; }
    
    let uv = vec2<f32>(f32(id.x) / 512.0, f32(id.y) / 512.0);
    let pos = uv * 2.0 - 1.0;
    
    var accum = 0.0;
    var weight = 0.0;
    
    // Sample from top layers (5 and 6)
    for (var layer = 5u; layer <= 6u; layer++) {
        let layer_offset = layer * 262144u;
        let base_u = u32(uv.x * 512.0);
        let base_v = u32(uv.y * 512.0);
        
        // 3x3 neighborhood
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let gx = clamp(i32(base_u) + dx, 0, 511);
                let gy = clamp(i32(base_v) + dy, 0, 511);
                let idx = atomicLoad(&spatialgrid[u32(gx + gy * 512) + layer_offset]);
                
                if (idx != 0xFFFFFFFFu && idx < arrayLength(&neurons)) {
                    let n = neurons[idx];
                    if (n.fatigue > 0.0) {  // Valid neuron
                        let d = distance(pos, n.pos);
                        let w = exp(-d * 50.0);
                        accum += n.voltage * w;
                        weight += w;
                    }
                }
            }
        }
    }
    
    let final_val = select(0.5, accum / weight, weight > 0.001);
    textureStore(predictiontex, coord, vec4<f32>(final_val, final_val, final_val, 1.0));
}