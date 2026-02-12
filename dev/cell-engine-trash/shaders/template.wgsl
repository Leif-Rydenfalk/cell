// --- TEMPLATE BRAIN NODE ---
// This shader uses the Dream Engine Protocol V1.

// The Framework automatically injects 'framework/common.rs' at the top.
// This gives you access to:
// - struct GlobalUniforms { time, dt, frame, mouse_pos, ... }
// - fn hash_u32, rand_f32, sigmoid, etc.

// 1. DATA SCHEMA
// Must align 16-byte boundaries. Total size: 48 bytes.
struct Neuron {
    semantic: vec4<u32>, // 16 bytes (SDR or Identity)
    pos: vec2<f32>,      // 8 bytes (-1.0 to 1.0)
    voltage: f32,        // 4 bytes
    prediction: f32,     // 4 bytes
    precision_val: f32,  // 4 bytes - Renamed from precision
    layer: u32,          // 4 bytes
    fatigue: f32,        // 4 bytes
    pad: f32,            // 4 bytes (Padding)
};

// 2. PARAMETERS
// Defined in blueprint.json "param_count".
// Access via params.my_val
struct NodeParams {
    learning_rate: f32,
    decay: f32,
    sensitivity: f32,
    noise_lvl: f32,
};

// 3. BINDINGS (Strict Order)

// Group 0: Local Node Data
@group(0) @binding(0) var<uniform> params: NodeParams;

// Ping-Pong Buffers
// If use_ping_pong is true: Binding 1 is READ ONLY (Previous State), Binding 2 is WRITE ONLY (Next State).
@group(0) @binding(1) var<storage, read> in_neurons: array<Neuron>;
@group(0) @binding(2) var<storage, read_write> out_neurons: array<Neuron>;

// Inputs (Defined in "input_textures" in JSON)
@group(0) @binding(3) var retina: texture_2d<f32>;

// Outputs (Defined in "output_textures" in JSON)
@group(0) @binding(4) var pred_out: texture_storage_2d<rgba32float, write>;

// Group 1: Global Engine Data
@group(1) @binding(0) var<uniform> global: GlobalParams;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= arrayLength(&out_neurons)) { return; }

    // Load State
    let n_old = in_neurons[idx];
    var n_new = n_old;

    // --- INITIALIZATION (Run once) ---
    if (global.frame < 1u) {
        let seed = idx * 763u;
        n_new.pos = vec2<f32>(rand_f32(seed), rand_f32(seed+1u)) * 2.0 - 1.0;
        n_new.voltage = 0.0;
        n_new.layer = u32(rand_f32(seed+2u) * 6.0);
    }

    // --- INPUT SENSING ---
    // Map neuron position (-1..1) to UV (0..1)
    let uv = n_new.pos * 0.5 + 0.5;
    let dim = vec2<f32>(textureDimensions(retina));
    let coord = vec2<i32>(uv * dim);
    
    // Sample texture (Red channel)
    let sensory_input = textureLoad(retina, coord, 0).r;

    // --- DYNAMICS ---
    // Simple leaky integrator
    let delta = sensory_input - n_new.prediction;
    n_new.voltage = mix(n_new.voltage, delta, params.learning_rate);
    n_new.voltage *= params.decay;

    // --- OUTPUT ---
    out_neurons[idx] = n_new;

    // Visualize output to texture
    let col = vec4<f32>(n_new.voltage, 0.0, 0.0, 1.0);
    textureStore(pred_out, coord, col);
}