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
const LATENT_VECS: u32 = 4u; // 16 dimensions (4 * vec4)

// --- HYPERPARAMETERS v3 ---
const BOND_RADIUS: f32 = 0.55;      // Max distance to listen
const REPULSION_RADIUS: f32 = 0.15; // "Personal Space" (Prevents singularity)
const INHIBITION_STR: f32 = 2.5;    // Strength of "Shut up neighbors"
const EXCITATION_STR: f32 = 1.8;    // Strength of "Fire together"
const TARGET_SPARSITY: f32 = 0.04;  // We want only 4% of neurons active
const HOMEOSTASIS_RATE: f32 = 0.05; // How fast threshold adapts

@group(0) @binding(0) var<storage, read_write> latent_in: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> latent_out: array<vec4<f32>>;

struct NeuronState {
    potential: f32,
    activation: f32,
    threshold: f32,
    refractory: f32,
    boredom: f32,     // High if variance is low
    avg_activity: f32, // Running average of activation
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(2) var<storage, read_write> state: array<NeuronState>;

@group(0) @binding(3) var input_tex: texture_2d<f32>;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(5) var debug_tex: texture_storage_2d<rgba8unorm, write>;

// --- MATH UTILS ---

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

    for (var i = 0u; i < LATENT_VECS; i++) {
        let h1 = hash3(idx * 13u + i);
        let h2 = hash3(idx * 27u + i);
        latent_in[idx * LATENT_VECS + i] = (vec4<f32>(h1, h2.x) - 0.5) * 2.0;
        latent_out[idx * LATENT_VECS + i] = (vec4<f32>(h2.y, h2.z, h1.x, h1.y) - 0.5) * 2.0;
    }

    state[idx].potential = 0.0;
    state[idx].activation = 0.0;
    state[idx].threshold = 0.8; // Start high
    state[idx].refractory = 0.0;
    state[idx].boredom = 0.0;
    state[idx].avg_activity = 0.0;
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
    let tex_dim = textureDimensions(input_tex);
    let sample_pos = vec2<i32>(i32(uv.x * f32(tex_dim.x)), i32(uv.y * f32(tex_dim.y)));
    let input_col = textureLoad(input_tex, sample_pos, 0).rgb;
    // Edge detection (simple difference from neighbor) - Neurons like change!
    let input_lum = dot(input_col, vec3<f32>(0.333));
    
    // 2. INTEGRATION (Excitatory vs Inhibitory)
    var excitation = 0.0;
    var inhibition = 0.0;
    var valid_bonds = 0.0;

    // Check spatial neighborhood (5x5)
    for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
            if (dx == 0 && dy == 0) { continue; }
            let nx = i32(lx) + dx;
            let ny = i32(ly) + dy;
            
            if (nx >= 0 && nx < i32(WIDTH) && ny >= 0 && ny < i32(HEIGHT)) {
                let n_idx = u32(ny) * WIDTH + u32(nx);
                let neighbor_act = state[n_idx].activation;
                
                if (neighbor_act > 0.05) {
                    let dist = get_latent_dist(my_idx, n_idx);
                    
                    // LATERAL INHIBITION:
                    // If neighbor is firing but we are NOT chemically bonded (dist is high),
                    // they scream at us to be quiet. This creates "contrast".
                    if (dist > BOND_RADIUS) {
                        inhibition += neighbor_act; 
                    } else {
                        // We are bonded.
                        // However, if we are TOO close (identical), we ignore them (redundancy filter)
                        if (dist > REPULSION_RADIUS) {
                            let bond = 1.0 - (dist / BOND_RADIUS);
                            excitation += neighbor_act * bond;
                            valid_bonds += 1.0;
                        }
                    }
                }
            }
        }
    }

    // 3. DYNAMICS
    var s = state[my_idx];
    
    // Total Input = Sensory + (Friends - Enemies)
    let net_input = input_lum * 0.5 + (excitation * EXCITATION_STR) - (inhibition * INHIBITION_STR);
    
    // Update Potential (Leaky Integrator)
    s.potential = s.potential * 0.85 + net_input * 0.2;
    
    // Refractory Period
    s.refractory = max(0.0, s.refractory - 0.1);

    // Fire?
    var fired = 0.0;
    if (s.potential > s.threshold && s.refractory <= 0.0) {
        fired = 1.0;
        s.potential = -0.5; // Hyperpolarization (dip below zero)
        s.refractory = 1.0; // Cannot fire for next 10 frames approx
    }
    
    // Smooth Activation for visual output
    s.activation = mix(s.activation, fired, 0.4);

    // 4. HOMEOSTASIS (The "Anti-Seizure" Mechanism)
    // If average activity > target, raise threshold.
    // If average activity < target, lower threshold.
    s.avg_activity = mix(s.avg_activity, fired, 0.01);
    let error = s.avg_activity - TARGET_SPARSITY;
    s.threshold += error * HOMEOSTASIS_RATE; 
    s.threshold = clamp(s.threshold, 0.1, 2.0); // Safety limits

    // 5. LEARNING (Latent Drift)
    let base_idx = my_idx * LATENT_VECS;
    let learn_rate = 0.02 * (1.0 + s.boredom); // Learn faster if bored
    
    if (fired > 0.5) {
        // Look at neighbors again
        for (var dy = -2; dy <= 2; dy++) {
            for (var dx = -2; dx <= 2; dx++) {
                if (dx == 0 && dy == 0) { continue; }
                let nx = i32(lx) + dx;
                let ny = i32(ly) + dy;
                if (nx >= 0 && nx < i32(WIDTH) && ny >= 0 && ny < i32(HEIGHT)) {
                    let n_idx = u32(ny) * WIDTH + u32(nx);
                    let n_act = state[n_idx].activation;
                    
                    if (n_act > 0.1) {
                        let dist = get_latent_dist(my_idx, n_idx);
                        
                        // HEBBIAN (Attraction): Pull IN towards neighbor's OUT
                        if (dist < BOND_RADIUS && dist > REPULSION_RADIUS) {
                            for (var i = 0u; i < LATENT_VECS; i++) {
                                let diff = latent_out[n_idx * LATENT_VECS + i] - latent_in[base_idx + i];
                                latent_in[base_idx + i] += diff * learn_rate;
                            }
                        }
                        
                        // ANTI-CLUMPING (Repulsion):
                        // If we are basically the same neuron, push away!
                        if (dist < REPULSION_RADIUS) {
                             for (var i = 0u; i < LATENT_VECS; i++) {
                                let dir = latent_in[base_idx + i] - latent_out[n_idx * LATENT_VECS + i];
                                latent_in[base_idx + i] += dir * learn_rate * 4.0; // Strong push
                            }
                        }
                    }
                }
            }
        }
        s.boredom = max(0.0, s.boredom - 0.1); // Found activity, not bored
    } else {
        // If I haven't fired in a long time, I get bored and drift randomly
        if (s.avg_activity < TARGET_SPARSITY * 0.1) {
             s.boredom = min(1.0, s.boredom + 0.001);
             for (var i = 0u; i < LATENT_VECS; i++) {
                 let r = hash3(my_idx + global.frame + i);
                 latent_in[base_idx + i] += (vec4<f32>(r.x, r.y, r.z, 0.0) - 0.5) * 0.01 * s.boredom;
             }
        }
    }
    
    // Normalize latent space to keep math stable
    if (global.frame % 30u == 0u) {
        for (var i = 0u; i < LATENT_VECS; i++) {
            latent_in[base_idx + i] = clamp(latent_in[base_idx + i], vec4<f32>(-2.0), vec4<f32>(2.0));
        }
    }

    state[my_idx] = s;

    // 6. VISUALIZATION
    // Red = Firing
    // Green = Threshold (High green = suppression/inhibition active)
    // Blue = Latent coordinate X (shows clustering)
    
    let l_vis = latent_in[base_idx].x * 0.5 + 0.5;
    
    let out_col = vec4<f32>(
        s.activation * 3.0, 
        (s.threshold - 0.1) * 0.5, 
        l_vis * s.activation, 
        1.0
    );
    
    textureStore(output_tex, vec2<i32>(i32(lx*4u), i32(ly*4u)), out_col);
    textureStore(debug_tex, vec2<i32>(i32(lx), i32(ly)), vec4<f32>(latent_in[base_idx].rgb * 0.5 + 0.5, 1.0));
}