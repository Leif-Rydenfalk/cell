use anyhow::Result;
use cell_sdk::cell_remote;
use std::time::Duration;

// Generates 'RendererService' client from source
// Fix: Use UpperCamelCase name 'RendererService' to match the struct used in main()
cell_remote!(RendererService = "renderer");

#[tokio::main]
async fn main() -> Result<()> {
    println!("[Brain] Connecting...");
    let mut retina = RendererService::connect().await?;
    println!("[Brain] Connected.");

    let mut pos = vec![0.0f32, 5.0, 10.0];

    loop {
        // get_input_state returns InputState struct defined in renderer
        if let Ok(input) = retina.get_input_state().await {
            let speed = 0.1;
            // KeyCode::W = 1, S = 3 (Based on renderer's KeyCode enum order)
            if input.keys_down.contains(&(1 as u16)) { pos[2] -= speed; }
            if input.keys_down.contains(&(3 as u16)) { pos[2] += speed; }

            retina.set_camera(
                pos.clone(),
                vec![pos[0], pos[1], pos[2] - 1.0],
                vec![0.0, 1.0, 0.0]
            ).await?;
        }
        
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
}