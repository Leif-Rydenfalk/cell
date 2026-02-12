pub const COMMON_WGSL: &str = r#"
// struct GlobalParams {
//     time: f32,
//     dt: f32,
//     frame: u32,
//     mouse_btn: u32,
//     mouse_pos: vec2<f32>,
//     screen_size: vec2<f32>,
// }

// // --- HASHING ---
// fn hash(x: u32) -> u32 {
//     var state = x;
//     state = state * 747796405u + 2891336453u;
//     let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
//     return (word >> 22u) ^ word;
// }

// fn rand_f32(seed: u32) -> f32 {
//     return f32(hash(seed)) / 4294967295.0;
// }

// fn rng_next(state: ptr<function, u32>) -> f32 {
//     let old = *state;
//     *state = old * 747796405u + 2891336453u;
//     let word = ((*state >> ((*state >> 28u) + 4u)) ^ *state) * 277803737u;
//     return f32((word >> 22u) ^ word) / 4294967295.0;
// }

// // --- MATH ---
// fn sigmoid(x: f32) -> f32 { return 1.0 / (1.0 + exp(-x)); }
// fn relu(x: f32) -> f32 { return max(0.0, x); }

// fn hamming(a: vec4<u32>, b: vec4<u32>) -> f32 {
//     let diff = a ^ b;
//     let bits = countOneBits(diff.x) + countOneBits(diff.y) + countOneBits(diff.z) + countOneBits(diff.w);
//     return max(0.0, 1.0 - f32(bits) / 128.0); 
// }
"#;
