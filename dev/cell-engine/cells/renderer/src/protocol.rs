use cell_sdk::protein;

#[protein(class = "RetinaContract")]
pub enum RenderCommand {
    // New: Define offscreen buffers for composition
    CreateTexture {
        id: String,
        width: u32,
        height: u32,
        format: String, // "rgba8", "depth32", etc
    },
    RegisterPass {
        id: String,
        shader_source: String,
        // New: Bindings
        inputs: Vec<String>,  // Textures to read (Bind Group 0)
        outputs: Vec<String>, // Textures to write (Render Targets). Empty = Screen.
        topology: String,
    },
    UpdateResource {
        id: String,
        data: Vec<u8>,
    },
    SpawnEntity {
        id: String,
        pass_id: String,
        resource_id: String,
        vertex_count: u32,
    },
    DespawnEntity {
        id: String,
    },
    SetCamera {
        position: [f32; 3],
        target: [f32; 3],
        up: [f32; 3],
    },
    GetInputState,
}

#[protein]
#[repr(u16)]
#[derive(Copy, PartialEq, Eq)]
pub enum KeyCode {
    Unknown = 0,
    W,
    A,
    S,
    D,
    Q,
    E,
    Space,
    Shift,
    Esc,
}

#[protein]
pub struct InputState {
    pub keys_down: Vec<KeyCode>,
    pub mouse_delta: [f32; 2],
}
