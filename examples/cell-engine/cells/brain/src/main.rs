use anyhow::Result;
use cell_sdk::*;
use std::time::Duration;

// DUPLICATED CONTRACT (Decoupled)
#[protein(class = "RetinaContract")]
pub enum RenderCommand {
    RegisterPass { id: String, shader_source: String, topology: String },
    UpdateResource { id: String, data: Vec<u8> },
    SpawnEntity { id: String, pass_id: String, resource_id: String, vertex_count: u32 },
    SetCamera { position: [f32; 3], target: [f32; 3], up: [f32; 3] },
    GetInputState,
}

#[protein]
#[repr(u16)]
#[derive(Copy, PartialEq, Eq)]
pub enum KeyCode {
    Unknown = 0,
    W, A, S, D, Q, E,
    Space, Shift, Esc
}

#[protein]
pub struct InputState {
    pub keys_down: Vec<KeyCode>,
    pub mouse_delta: [f32; 2],
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("[Brain] Online.");
    
    // Connect to Renderer
    let mut retina = Synapse::grow("renderer").await?;
    
    let mut pos = [0.0f32, 5.0, 10.0];
    let mut yaw = -90.0f32;
    let mut pitch = 0.0f32;

    loop {
        // Request Inputs
        if let Ok(v) = retina.fire(RenderCommand::GetInputState).await {
            if let Ok(input) = cell_sdk::rkyv::from_bytes::<InputState>(v.as_slice()) {
                
                // Simple Logic
                let speed = 0.1;
                if input.keys_down.contains(&KeyCode::W) { pos[2] -= speed; }
                if input.keys_down.contains(&KeyCode::S) { pos[2] += speed; }

                yaw += input.mouse_delta[0] * 0.1;
                pitch -= input.mouse_delta[1] * 0.1;
                
                // Update Camera
                retina.fire(RenderCommand::SetCamera {
                    position: pos,
                    target: [pos[0], pos[1], pos[2] - 1.0],
                    up: [0.0, 1.0, 0.0],
                }).await?;
            }
        }
        
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}