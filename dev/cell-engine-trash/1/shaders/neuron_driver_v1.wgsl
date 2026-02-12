struct Global {
    time: f32,
    dt: f32,
    frame: u32,
    _pad1: u32,
    mouse: vec4<f32>,
    screen: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

const TOTAL_BLOCKS: u32 = 19200u; // 160x120
const LEARNING_RATE: f32 = 0.05;
const PREDICT_RATE: f32 = 0.02;   // Learning rate for the world model
const DECAY: f32 = 0.999;         // Weight decay (regularization)
const TIME_CONSTANT: f32 = 0.2;   // Liquid time constant (tau)

@group(1) @binding(0) var<uniform> global: Global;

// --- MEMORY ARCHITECTURE ---
// 1. ENCODER: Input(16px) -> Latent(4ch)
@group(0) @binding(0) var<storage, read_write> w_encoder: array<f32>; // 19200 * 16 * 4
// 2. DECODER: Latent(4ch) -> Output(16px)
@group(0) @binding(1) var<storage, read_write> w_decoder: array<f32>; // 19200 * 4 * 16
// 3. WORLD MODEL: Latent(t) -> Latent(t+1) (4x4 Matrix per block)
@group(0) @binding(2) var<storage, read_write> w_predict: array<f32>; // 19200 * 4 * 4
// 4. LIQUID STATE: Persistent memory of the neuron potentials
@group(0) @binding(3) var<storage, read_write> state: array<vec4<f32>>; // 19200

@group(0) @binding(4) var input_tex: texture_2d<f32>;
@group(0) @binding(5) var output_tex: texture_storage_2d<rgba8unorm, write>;     
@group(0) @binding(6) var latent_tex: texture_storage_2d<rgba8unorm, write>;

@group(0) @binding(10) var display_tex: texture_2d<f32>;
@group(0) @binding(11) var display_sampler: sampler;

// Fast hash for init
fn hash(p: u32) -> f32 {
    let p1 = p * 747796405u + 2891336453u;
    let p2 = ((p1 >> ((p1 >> 28u) + 4u)) ^ p1) * 277803737u;
    return f32((p2 >> 22u) ^ p2) / 4294967295.0;
}

// Activation: Swish (x * sigmoid(x)) - creates complex gradients
fn act(x: f32) -> f32 { return x / (1.0 + exp(-x)); }
fn d_act(x: f32) -> f32 { 
    let sig = 1.0 / (1.0 + exp(-x));
    return sig + x * sig * (1.0 - sig);
}

@compute @workgroup_size(64)
fn cs_init(@builtin(global_invocation_id) id: vec3<u32>) {
    if (global.frame > 1u) { return; }
    let idx = id.x;
    
    // Init Encoder/Decoder (Identity-ish)
    if (idx < TOTAL_BLOCKS * 64u) {
        w_encoder[idx] = (hash(idx) - 0.5) * 0.1;
        w_decoder[idx] = (hash(idx + 12345u) - 0.5) * 0.1;
    }
    
    // Init World Model Matrix (Identity + Noise)
    // We want the state to persist, so diagonal should be close to 1.0
    if (idx < TOTAL_BLOCKS * 16u) {
        let r = idx % 4u;
        let c = (idx / 4u) % 4u;
        var val = (hash(idx + 9999u) - 0.5) * 0.1;
        if (r == c) { val += 0.8; } // Stability bias
        w_predict[idx] = val;
    }

    // Clear State
    if (idx < TOTAL_BLOCKS) {
        state[idx] = vec4<f32>(0.0);
    }
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let lx = id.x; 
    let ly = id.y;
    
    if (lx >= 160u || ly >= 120u) { return; }

    let block_id = ly * 160u + lx;
    let base_enc_idx = block_id * 64u; // 16 pixels * 4 latents
    let base_dec_idx = block_id * 64u; // 4 latents * 16 pixels
    let base_pre_idx = block_id * 16u; // 4 * 4 matrix
    
    let ox = lx * 4u;
    let oy = ly * 4u;

    // --- 1. SENSORY INTAKE ---
    // Cache input patch
    var input_cache: array<vec3<f32>, 16>;
    for (var i = 0u; i < 16u; i++) {
        let kx = i % 4u;
        let ky = i / 4u;
        input_cache[i] = textureLoad(input_tex, vec2<i32>(i32(ox + kx), i32(oy + ky)), 0).rgb;
    }

    // Encode: Project 16px -> 4 Latent targets
    var sensor_drive = vec4<f32>(0.0);
    for (var p = 0u; p < 16u; p++) {
        let pixel_lum = dot(input_cache[p], vec3<f32>(0.333)); // Grayscale driver for now
        for (var c = 0u; c < 4u; c++) {
            sensor_drive[c] += pixel_lum * w_encoder[base_enc_idx + p * 4u + c];
        }
    }

    // --- 2. LIQUID DYNAMICS (ODE Solver) ---
    // Retrieve previous state (memory)
    let prev_state = state[block_id];
    
    // Calculate internal prediction (What did the World Model think would happen?)
    // Latent_Hat = Matrix * Prev_State
    var prediction = vec4<f32>(0.0);
    for (var r = 0u; r < 4u; r++) {
        for (var c = 0u; c < 4u; c++) {
             prediction[r] += prev_state[c] * w_predict[base_pre_idx + r * 4u + c];
        }
    }

    // Update State:
    // dState = (Sensory_Input - State) + (Prediction - State)
    // This balances "What I see" vs "What I expected"
    let new_potential = prev_state + (sensor_drive - prev_state + (prediction - prev_state) * 0.5) * TIME_CONSTANT;
    
    // Non-linearity (Firing Rate)
    let firing_rate = vec4<f32>(
        act(new_potential.x), act(new_potential.y),
        act(new_potential.z), act(new_potential.w)
    );
    
    // Store new liquid state
    state[block_id] = new_potential;
    
    // Visualizing the internal brain state
    textureStore(latent_tex, vec2<i32>(i32(lx), i32(ly)), firing_rate);

    // --- 3. DECODE & RECONSTRUCT ---
    var error_accum = vec4<f32>(0.0);

    for (var p = 0u; p < 16u; p++) {
        var recon_val = 0.0;
        for (var c = 0u; c < 4u; c++) {
            recon_val += firing_rate[c] * w_decoder[base_dec_idx + c * 16u + p];
        }
        
        // Diff (RENAMED 'target' -> 'tgt_lum')
        let tgt_lum = dot(input_cache[p], vec3<f32>(0.333));
        let err = tgt_lum - recon_val;
        
        // Backprop to Decoder Weights
        for (var c = 0u; c < 4u; c++) {
            let w_idx = base_dec_idx + c * 16u + p;
            let dw = err * firing_rate[c];
            w_decoder[w_idx] = w_decoder[w_idx] + dw * LEARNING_RATE;
            
            // Accumulate error for Latent update
            error_accum[c] += err * w_decoder[w_idx];
        }
        
        // Draw Output
        let kx = p % 4u;
        let ky = p / 4u;
        textureStore(output_tex, vec2<i32>(i32(ox+kx), i32(oy+ky)), vec4<f32>(vec3<f32>(recon_val), 1.0));
    }

    // --- 4. PREDICTIVE LEARNING (The World Model Update) ---
    // We want the PREDICTION from t-1 to match the REALITY at t.
    // Prediction Error = Current_State - Predicted_State
    let pred_err = firing_rate - prediction;

    // Update World Model Matrix
    // "Hebbian-ish" update: connect active past neurons to active current errors
    for (var r = 0u; r < 4u; r++) { // Target row
        for (var c = 0u; c < 4u; c++) { // Source col
             let dw_pred = pred_err[r] * prev_state[c];
             let idx = base_pre_idx + r * 4u + c;
             w_predict[idx] = (w_predict[idx] + dw_pred * PREDICT_RATE) * DECAY;
        }
    }

    // Update Encoder
    // Propagate reconstruction error through the activation derivative
    for (var p = 0u; p < 16u; p++) {
        let px_val = dot(input_cache[p], vec3<f32>(0.333));
        for (var c = 0u; c < 4u; c++) {
             let grad = error_accum[c] * d_act(new_potential[c]);
             let idx = base_enc_idx + p * 4u + c;
             w_encoder[idx] = (w_encoder[idx] + grad * px_val * LEARNING_RATE) * DECAY;
        }
    }
}

// --- RENDER ---
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> VertexOutput {
    var o: VertexOutput;
    let uv = vec2<f32>(f32((i<<1u)&2u), f32(i&2u));
    o.clip_position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return o;
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(display_tex, display_sampler, in.uv);
}