use anyhow::Result;
use cell_sdk::*; 
use std::time::{Duration, Instant};

// Exact duplicate of renderer::protocol::RenderCommand
#[protein(class = "RetinaContract")]
pub enum RenderCommand {
    RegisterPass {
        id: String,
        shader_source: String,
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
    SetBackgroundColor {
        color: [f32; 4],
    },
    GetInputState,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(check_bytes)]
#[archive(crate = "cell_sdk::rkyv")]
pub struct InputState {
    pub keys_down: Vec<String>,
    pub mouse_delta: [f32; 2],
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("[Brain] Online. Auto-connecting...");
    Membrane::bind_background("brain", |_| async { Ok(Vesicle::new()) }).await?;

    let mut retina = Synapse::grow("renderer").await?;
    let _atlas = Synapse::grow("atlas").await?; 

    println!("[Brain] Assembled. Initializing RPG Controller...");

    // Setup Camera
    let mut pos = [0.0f32, 20.0, 20.0];
    let mut yaw = -90.0f32; // Facing -Z
    let mut pitch = -45.0f32;
    
    // Loop
    loop {
        // 1. Get Input
        let v = retina.fire(RenderCommand::GetInputState).await?;
        if let Ok(input) = cell_sdk::rkyv::from_bytes::<InputState>(v.as_slice()) {
            // 2. Update Logic
            let speed = 0.5;
            let sensitivity = 0.1;
            
            // Mouse Look
            yaw += input.mouse_delta[0] * sensitivity;
            pitch -= input.mouse_delta[1] * sensitivity;
            pitch = pitch.clamp(-89.0, 89.0);
            
            // Calc forward vector
            let yaw_rad = yaw.to_radians();
            let pitch_rad = pitch.to_radians();
            let front = [
                yaw_rad.cos() * pitch_rad.cos(),
                pitch_rad.sin(),
                yaw_rad.sin() * pitch_rad.cos()
            ];
            let right = [
                (yaw_rad - std::f32::consts::FRAC_PI_2).cos(),
                0.0,
                (yaw_rad - std::f32::consts::FRAC_PI_2).sin()
            ];
            
            // Keyboard Move (Simple string check)
            for key in &input.keys_down {
                if key.contains("KeyW") {
                    pos[0] += front[0] * speed;
                    pos[1] += front[1] * speed;
                    pos[2] += front[2] * speed;
                }
                if key.contains("KeyS") {
                    pos[0] -= front[0] * speed;
                    pos[1] -= front[1] * speed;
                    pos[2] -= front[2] * speed;
                }
                if key.contains("KeyA") {
                    pos[0] -= right[0] * speed;
                    pos[1] -= right[1] * speed;
                    pos[2] -= right[2] * speed;
                }
                if key.contains("KeyD") {
                    pos[0] += right[0] * speed;
                    pos[1] += right[1] * speed;
                    pos[2] += right[2] * speed;
                }
            }
            
            // 3. Update Camera
            retina.fire(RenderCommand::SetCamera {
                position: pos,
                target: [pos[0] + front[0], pos[1] + front[1], pos[2] + front[2]],
                up: [0.0, 1.0, 0.0],
            }).await?;
        }
        
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}