struct Global {
    time: f32,
    dt: f32,
    frame: u32,
    mouse: vec4<f32>,
    screen: vec2<f32>,
}

@group(1) @binding(0) var<uniform> global: Global;

const WIDTH: u32 = 160u;
const HEIGHT: u32 = 120u;
const N_NEURONS: u32 = 19200u;
const LATENT_VECS: u32 = 4u;
const SYNAPSE_COUNT: u32 = 8u; // Connections per neuron

// Tuning
const TARGET_SPARSITY: f32 = 0.05;
const PLASTICITY_RATE: f32 = 0.01; // How fast connections change

@group(0) @binding(0) var<storage, read_write> latent_in: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> latent_out: array<vec4<f32>>;

struct NeuronState {
    potential: f32,
    activation: f32,
    threshold: f32,
    refractory: f32,
    avg_activity: f32,
    boredom: f32,
    _pad1: f32, 
    _pad2: f32,
}
@group(0) @binding(2) var<storage, read_write> state: array<NeuronState>;

// The Connectivity Graph
// Each neuron stores 8 indices of other neurons it listens to.
@group(0) @binding(3) var<storage, read_write> synapse_map: array<array<u32, 8>>;

@group(0) @binding(4) var input_tex: texture_2d<f32>;
@group(0) @binding(5) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(6) var debug_tex: texture_storage_2d<rgba8unorm, write>;

// --- UTILS ---
fn hash(p: u32) -> f32 {
    let p1 = p * 747796405u + 2891336453u;
    let p2 = ((p1 >> ((p1 >> 28u) + 4u)) ^ p1) * 277803737u;
    return f32((p2 >> 22u) ^ p2) / 4294967295.0;
}
fn hash3(p: u32) -> vec3<f32> {
    return vec3<f32>(hash(p), hash(p + 1u), hash(p + 2u));
}

fn get_latent_dist(idx_a: u32, idx_b: u32) -> f32 {
    var dist_sq = 0.0;
    let base_a = idx_a * LATENT_VECS;
    let base_b = idx_b * LATENT_VECS;
    for (var i = 0u; i < LATENT_VECS; i++) {
        let d = latent_in[base_a + i] - latent_out[base_b + i];
        dist_sq += dot(d, d);
    }
    return sqrt(dist_sq);
}

// --- INIT ---
@compute @workgroup_size(64)
fn cs_init(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= N_NEURONS || global.frame > 1u) { return; }

    // Init Latents
    for (var i = 0u; i < LATENT_VECS; i++) {
        let h = hash3(idx * 10u + i);
        latent_in[idx * LATENT_VECS + i] = (vec4<f32>(h, hash(idx+i)) - 0.5) * 2.0;
        let h2 = hash3(idx * 20u + i);
        latent_out[idx * LATENT_VECS + i] = (vec4<f32>(h2, hash(idx+i+1u)) - 0.5) * 2.0;
    }

    // Init State
    state[idx].potential = 0.0;
    state[idx].activation = 0.0;
    state[idx].threshold = 0.5;
    state[idx].refractory = 0.0;
    state[idx].boredom = 0.0;

    // Init Synapses (Connect to random local neighbors initially)
    let lx = idx % WIDTH;
    let ly = idx / WIDTH;
    for (var i = 0u; i < SYNAPSE_COUNT; i++) {
        let rx = clamp(i32(lx) + i32(hash(idx + i)*10.0 - 5.0), 0, i32(WIDTH)-1);
        let ry = clamp(i32(ly) + i32(hash(idx + i + 50u)*10.0 - 5.0), 0, i32(HEIGHT)-1);
        synapse_map[idx][i] = u32(ry) * WIDTH + u32(rx);
    }
}

// --- MAIN LOOP ---
@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let lx = id.x; 
    let ly = id.y;
    if (lx >= WIDTH || ly >= HEIGHT) { return; }

    let my_idx = ly * WIDTH + lx;
    let base_idx = my_idx * LATENT_VECS;
    var s = state[my_idx];

    // 1. GATHER INPUT (Synaptic Integration)
    // We only look at our specific list of connections, not just spatial neighbors
    var synaptic_drive = 0.0;
    var inhibition = 0.0;

    for (var i = 0u; i < SYNAPSE_COUNT; i++) {
        let target_idx = synapse_map[my_idx][i];
        let target_act = state[target_idx].activation;
        
        if (target_act > 0.05) {
            let dist = get_latent_dist(my_idx, target_idx);
            
            // If latent concepts match: Excite
            // If latent concepts mismatch: Inhibit (Lateral Inhibition via long range!)
            if (dist < 0.6) {
                synaptic_drive += target_act * (1.0 - dist);
            } else {
                inhibition += target_act * 0.5;
            }
        }
    }

    // Sensory Input
    let uv = vec2<f32>(f32(lx)/f32(WIDTH), f32(ly)/f32(HEIGHT));
    let cam_col = textureLoad(input_tex, vec2<i32>(i32(lx * 4u), i32(ly * 4u)), 0).rgb; // Approx mapping
    let sensory = dot(cam_col, vec3<f32>(0.333));

    // 2. FIRE LOGIC
    let net_input = sensory * 0.3 + synaptic_drive * 1.5 - inhibition * 1.0;
    
    s.potential = s.potential * 0.9 + net_input * 0.2;
    s.refractory = max(0.0, s.refractory - 0.1);
    
    var fired = 0.0;
    if (s.potential > s.threshold && s.refractory <= 0.0) {
        fired = 1.0;
        s.potential = -0.2;
        s.refractory = 2.0;
    }
    
    s.activation = mix(s.activation, fired, 0.5);
    
    // Homeostasis
    s.avg_activity = mix(s.avg_activity, fired, 0.01);
    s.threshold += (s.avg_activity - TARGET_SPARSITY) * 0.05;
    s.threshold = clamp(s.threshold, 0.1, 2.0);

    // 3. NEUROPLASTICITY & REWIRING (The "Small World" logic)
    // Every frame, we consider changing ONE synapse
    let slot_to_update = u32(global.frame + my_idx) % SYNAPSE_COUNT;
    let current_target = synapse_map[my_idx][slot_to_update];
    let current_dist = get_latent_dist(my_idx, current_target);
    
    // Chance to rewire depends on how "bad" (distant) the current connection is
    let rewire_chance = smoothstep(0.4, 1.5, current_dist) * 0.1 + 0.001;
    
    if (hash(my_idx + global.frame) < rewire_chance) {
        // STRATEGY 1: Spatial Grounding (Reconnect to physical neighbor)
        // Helps maintain local coherence
        let rx = clamp(i32(lx) + i32(hash(my_idx)*6.0 - 3.0), 0, i32(WIDTH)-1);
        let ry = clamp(i32(ly) + i32(hash(my_idx+1u)*6.0 - 3.0), 0, i32(HEIGHT)-1);
        let candidate_local = u32(ry) * WIDTH + u32(rx);
        
        // STRATEGY 2: Transitive Linking (Friend of a Friend)
        // Connect to one of my CURRENT connection's connections.
        // This causes chains to grow.
        let random_synapse = synapse_map[my_idx][u32(hash(my_idx)*f32(SYNAPSE_COUNT))];
        let candidate_transitive = synapse_map[random_synapse][u32(hash(my_idx+2u)*f32(SYNAPSE_COUNT))];
        
        // Decide which to pick based on latent alignment
        let dist_local = get_latent_dist(my_idx, candidate_local);
        let dist_trans = get_latent_dist(my_idx, candidate_transitive);
        
        if (dist_trans < dist_local && dist_trans < current_dist) {
            synapse_map[my_idx][slot_to_update] = candidate_transitive; // Extend chain!
        } else if (dist_local < current_dist) {
            synapse_map[my_idx][slot_to_update] = candidate_local; // Retract to local
        }
    }

    // 4. LATENT DRIFT (Hebbian)
    if (fired > 0.5) {
        for (var i = 0u; i < SYNAPSE_COUNT; i++) {
            let target = synapse_map[my_idx][i];
            if (state[target].activation > 0.1) {
                // Pull my INPUT towards their OUTPUT
                for (var k = 0u; k < LATENT_VECS; k++) {
                     let diff = latent_out[target * LATENT_VECS + k] - latent_in[base_idx + k];
                     latent_in[base_idx + k] += diff * 0.05;
                }
            }
        }
    }
    
    // Normalize
    if (global.frame % 60u == 0u) {
        for (var k = 0u; k < LATENT_VECS; k++) {
            latent_in[base_idx + k] = clamp(latent_in[base_idx + k], vec4<f32>(-2.0), vec4<f32>(2.0));
        }
    }

    state[my_idx] = s;

    // 5. VISUALIZATION
    // We visualize the long-range connections by coloring based on Latent Coords
    // If two distant pixels have the same color, they are "entangled"
    
    let latent_color = latent_in[base_idx].rgb * 0.5 + 0.5;
    let activity_vis = s.activation;
    
    // Output: RGB = Latent Concept, Alpha = Activity
    // This way, you see "Ideas" moving across the screen, not just white noise.
    let final_col = vec4<f32>(latent_color * (0.2 + activity_vis * 2.0), 1.0);
    
    textureStore(output_tex, vec2<i32>(i32(lx*4u), i32(ly*4u)), final_col);
    textureStore(debug_tex, vec2<i32>(i32(lx), i32(ly)), vec4<f32>(latent_color, 1.0));
}