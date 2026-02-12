struct Global {
    time: f32,
    dt: f32,
    frame: u32,
    mouse: vec4<f32>,
    screen: vec2<f32>,
}

@group(1) @binding(0) var<uniform> global: Global;

// 160x120 Grid
const WIDTH: u32 = 160u;
const HEIGHT: u32 = 120u;
const N_NEURONS: u32 = 19200u; // 160 * 120

// Latent Dimensions per neuron (We use 16 dimensions = 4 vec4s)
const LATENT_VECS: u32 = 4u; 

// Tuning
const LATENT_RADIUS: f32 = 0.6; // Distance to form a bond
const LEARNING_RATE: f32 = 0.05;
const DECAY: f32 = 0.90;
const NOISE_RATE: f32 = 0.01;

// --- MEMORY LAYOUT ---

// Latent Space: 16 Dimensions per neuron
// Stored as flattened array of vec4s. Index = neuron_id * 4 + vec_index
@group(0) @binding(0) var<storage, read_write> latent_in: array<vec4<f32>>;  // "What I listen to"
@group(0) @binding(1) var<storage, read_write> latent_out: array<vec4<f32>>; // "What I say"

struct NeuronState {
    potential: f32,
    activation: f32,     // The firing rate
    threshold: f32,      // Dynamic threshold
    excitement: f32,     // Emotional state (found a pattern)
    loneliness: f32,     // Emotional state (no neighbors)
    spike_timer: f32,    // Refractory period
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(2) var<storage, read_write> state: array<NeuronState>;

@group(0) @binding(3) var input_tex: texture_2d<f32>;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(5) var debug_tex: texture_storage_2d<rgba8unorm, write>;

// --- UTILS ---

// Hash for RNG
fn hash(p: u32) -> f32 {
    let p1 = p * 747796405u + 2891336453u;
    let p2 = ((p1 >> ((p1 >> 28u) + 4u)) ^ p1) * 277803737u;
    return f32((p2 >> 22u) ^ p2) / 4294967295.0;
}

fn hash3(p: u32) -> vec3<f32> {
    return vec3<f32>(hash(p), hash(p + 1u), hash(p + 2u));
}

// Distance in 16D Latent Space
fn get_latent_dist(idx_a: u32, idx_b: u32) -> f32 {
    var dist_sq = 0.0;
    // Compare A's INPUT to B's OUTPUT
    // (Does A want to listen to B?)
    let base_a = idx_a * LATENT_VECS;
    let base_b = idx_b * LATENT_VECS;
    
    for (var i = 0u; i < LATENT_VECS; i++) {
        let d = latent_in[base_a + i] - latent_out[base_b + i];
        dist_sq += dot(d, d);
    }
    return sqrt(dist_sq);
}

// Activation Function
fn sigmoid(x: f32) -> f32 {
    return 1.0 / (1.0 + exp(-x * 4.0));
}

// --- INIT ---

@compute @workgroup_size(64)
fn cs_init(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= N_NEURONS || global.frame > 1u) { return; }

    // Init Latents with random high-dim vectors
    // Normalize them somewhat to stay in -1 to 1 range
    for (var i = 0u; i < LATENT_VECS; i++) {
        let h1 = hash3(idx * 10u + i);
        let h2 = hash3(idx * 20u + i);
        latent_in[idx * LATENT_VECS + i] = vec4<f32>(h1 - 0.5, h2.x - 0.5);
        latent_out[idx * LATENT_VECS + i] = vec4<f32>(h2.y - 0.5, h2.z - 0.5, h1.y - 0.5, h1.z - 0.5);
    }

    state[idx].potential = 0.0;
    state[idx].activation = 0.0;
    state[idx].threshold = 0.5 + hash(idx) * 0.2;
    state[idx].excitement = 0.0;
    state[idx].loneliness = 1.0;
    state[idx].spike_timer = 0.0;
}

// --- MAIN LOOP ---

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let lx = id.x; 
    let ly = id.y;
    if (lx >= WIDTH || ly >= HEIGHT) { return; }

    let my_idx = ly * WIDTH + lx;
    let uv = vec2<f32>(f32(lx) / f32(WIDTH), f32(ly) / f32(HEIGHT));

    // 1. SENSORY INPUT
    // Map screen position to texture coordinates
    let tex_dim = textureDimensions(input_tex);
    let sample_pos = vec2<i32>(i32(uv.x * f32(tex_dim.x)), i32(uv.y * f32(tex_dim.y)));
    let input_color = textureLoad(input_tex, sample_pos, 0).rgb;
    let sensory_drive = dot(input_color, vec3<f32>(0.299, 0.587, 0.114)); // Grayscale intensity

    // 2. SYNAPTIC INTEGRATION (The "Listening" Phase)
    // Checking ALL neurons is O(N^2) = Slow.
    // Optimization: Check local neighbors + Random long-range sampling
    
    var synaptic_input = 0.0;
    var valid_connections = 0.0;
    
    // 2a. Check spatial neighbors (The "Renderer" logic - meaningful early connections)
    for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
            if (dx == 0 && dy == 0) { continue; }
            let nx = i32(lx) + dx;
            let ny = i32(ly) + dy;
            
            if (nx >= 0 && nx < i32(WIDTH) && ny >= 0 && ny < i32(HEIGHT)) {
                let neighbor_idx = u32(ny) * WIDTH + u32(nx);
                let dist = get_latent_dist(my_idx, neighbor_idx);
                
                // If concepts align (distance is small), we form a bond
                if (dist < LATENT_RADIUS) {
                    let weight = (1.0 - dist / LATENT_RADIUS);
                    synaptic_input += state[neighbor_idx].activation * weight;
                    valid_connections += 1.0;
                }
            }
        }
    }

    // 2b. Check random long-range neurons (The "Association" logic - forming new ideas)
    // Sample 8 random neurons per frame
    for (var k = 0u; k < 8u; k++) {
        let rnd = hash(my_idx + global.frame + k * 1920u);
        let neighbor_idx = u32(rnd * f32(N_NEURONS));
        if (neighbor_idx == my_idx) { continue; }
        
        let dist = get_latent_dist(my_idx, neighbor_idx);
        if (dist < LATENT_RADIUS) {
            let weight = (1.0 - dist / LATENT_RADIUS);
            synaptic_input += state[neighbor_idx].activation * weight;
            valid_connections += 1.0;
        }
    }

    // 3. NEURON DYNAMICS
    var s = state[my_idx];
    
    // Integrate: Potential = Old * Decay + Sensory + Synaptic
    let input_sum = sensory_drive * 0.5 + synaptic_input * 1.5;
    s.potential = s.potential * DECAY + input_sum * 0.1;
    
    // Refractory period
    s.spike_timer = max(0.0, s.spike_timer - global.dt);
    
    // Fire?
    if (s.potential > s.threshold && s.spike_timer <= 0.0) {
        s.activation = 1.0;
        s.potential = 0.0; // Reset
        s.spike_timer = 0.2; // 200ms refractory
        s.excitement = min(1.0, s.excitement + 0.1);
    } else {
        s.activation = s.activation * 0.8; // Fast decay of output pulse
        s.excitement = s.excitement * 0.99;
    }

    // Update Emotions
    // Loneliness = No valid connections
    let current_loneliness = 1.0 / (1.0 + valid_connections);
    s.loneliness = mix(s.loneliness, current_loneliness, 0.1);
    
    // Dynamic Threshold (Homeostasis)
    // If firing too much, raise threshold. If silent, lower it.
    if (s.activation > 0.5) {
        s.threshold += 0.01;
    } else {
        s.threshold = max(0.1, s.threshold - 0.001);
    }

    state[my_idx] = s;

    // 4. LEARNING & PLASTICITY (The "Drifting" Phase)
    // Neurons move in latent space based on activity
    
    let base_idx = my_idx * LATENT_VECS;
    
    // Drift Logic:
    // If I am active, move my INPUT latent towards the OUTPUT latent of those who fed me.
    // If I am lonely, drift randomly (Exploration).
    
    let drift_speed = LEARNING_RATE * (1.0 + s.loneliness); // Learn faster if lonely
    
    // Random Exploration Vector
    var noise = vec4<f32>(0.0);
    
    if (s.activation > 0.1) {
        // Habbian Learning: Re-scan neighbors to pull latents closer
        // (Simplified: We only check spatial neighbors again for the update to save perf)
         for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let nx = i32(lx) + dx;
                let ny = i32(ly) + dy;
                if (nx >= 0 && nx < i32(WIDTH) && ny >= 0 && ny < i32(HEIGHT)) {
                    let n_idx = u32(ny) * WIDTH + u32(nx);
                    if (state[n_idx].activation > 0.1) {
                        // Move My INPUT towards Their OUTPUT
                        for (var i = 0u; i < LATENT_VECS; i++) {
                            let diff = latent_out[n_idx * LATENT_VECS + i] - latent_in[base_idx + i];
                            latent_in[base_idx + i] += diff * drift_speed * 0.05;
                            
                            // Also align my OUTPUT to be similar to theirs (cluster formation)
                            let diff_out = latent_out[n_idx * LATENT_VECS + i] - latent_out[base_idx + i];
                            latent_out[base_idx + i] += diff_out * drift_speed * 0.01;
                        }
                    }
                }
            }
        }
    } else if (s.loneliness > 0.8) {
        // High exploration noise if lonely
        for (var i = 0u; i < LATENT_VECS; i++) {
             let r = hash3(my_idx + global.frame + i);
             latent_in[base_idx + i] += (vec4<f32>(r.x, r.y, r.z, 0.0) - 0.5) * NOISE_RATE;
             latent_out[base_idx + i] += (vec4<f32>(r.z, r.y, r.x, 0.0) - 0.5) * NOISE_RATE;
        }
    }
    
    // Normalize Latents (Keep them roughly in -1..1 sphere to prevent explosion)
    if (global.frame % 10u == 0u) {
        for (var i = 0u; i < LATENT_VECS; i++) {
            latent_in[base_idx + i] = clamp(latent_in[base_idx + i], vec4<f32>(-2.0), vec4<f32>(2.0));
            latent_out[base_idx + i] = clamp(latent_out[base_idx + i], vec4<f32>(-2.0), vec4<f32>(2.0));
        }
    }

    // 5. VISUALIZATION
    // Output 1: Firing Activity (Green) + Loneliness (Blue) + Excitement (Red)
    let color = vec4<f32>(
        s.activation * 2.0,     // R: Firing
        s.loneliness,           // G: Lonely
        s.excitement,           // B: Pattern found
        1.0
    );
    textureStore(output_tex, vec2<i32>(i32(lx * 4u), i32(ly * 4u)), color);
    
    // Debug Output: Visualize the first 3 dimensions of the Input Latent Space
    // This allows us to see "Concept Clusters" forming.
    // Similar colors = Similar concepts.
    let l_vis = latent_in[base_idx].rgb * 0.5 + 0.5;
    textureStore(debug_tex, vec2<i32>(i32(lx), i32(ly)), vec4<f32>(l_vis, 1.0));
}