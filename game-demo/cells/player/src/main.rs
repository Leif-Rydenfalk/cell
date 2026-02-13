//! Player Cell - 6DOF Camera Controller with proper input
//!
//! This cell:
//! 1. Connects to renderer for input
//! 2. Provides 6DOF camera control (WASD + Mouse)
//! 3. FIXED: Mouse delta accumulation bug!
//! 4. ADDED: Press 'R' to reset camera position

use anyhow::Result;
use cell_sdk::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Duration;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};

cell_remote!(Renderer = "renderer");

// Key codes from winit
const KEY_W: u16 = 17;  // W
const KEY_A: u16 = 30;  // A
const KEY_S: u16 = 31;  // S
const KEY_D: u16 = 32;  // D
const KEY_SPACE: u16 = 57;  // Space
const KEY_LSHIFT: u16 = 42; // Left Shift
const KEY_R: u16 = 19;      // R - Reset camera
const KEY_ESC: u16 = 1;     // Escape
const KEY_Q: u16 = 16;      // Q
const KEY_E: u16 = 18;      // E

// Default camera position and orientation
const DEFAULT_POSITION: [f32; 3] = [20.0, 15.0, 40.0];
const DEFAULT_YAW: f32 = -PI / 4.0;  // 45 degrees left
const DEFAULT_PITCH: f32 = -0.3;     // Slightly down

#[derive(Clone)]
struct CameraState {
    position: [f32; 3],
    target: [f32; 3],
    up: [f32; 3],
    fov: f32,
    near: f32,
    far: f32,
    speed: f32,
    mouse_sensitivity: f32,
    yaw: f32,
    pitch: f32,
    first_mouse: bool,
}

impl CameraState {
    fn reset(&mut self) {
        self.position = DEFAULT_POSITION;
        self.yaw = DEFAULT_YAW;
        self.pitch = DEFAULT_PITCH;
        self.first_mouse = false;
        tracing::info!("[Player] 🔄 Camera reset to default position");
    }
    
    fn default() -> Self {
        Self {
            position: DEFAULT_POSITION,
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov: 60.0,
            near: 0.1,
            far: 1000.0,
            speed: 20.0,
            mouse_sensitivity: 0.003,
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            first_mouse: false,
        }
    }
}

#[service]
#[derive(Clone)]
struct PlayerService {
    camera: Arc<Mutex<CameraState>>,
    renderer: Arc<Mutex<Option<Renderer::Client>>>,
    frame_counter: Arc<AtomicU64>,
    last_reset_state: Arc<Mutex<bool>>, // Track if R key was just pressed
}

#[handler]
impl PlayerService {
    async fn tick(&self, _req: ()) -> Result<()> {
        // Connect to renderer if needed
        {
            let mut renderer = self.renderer.lock().await;
            if renderer.is_none() {
                match Renderer::Client::connect().await {
                    Ok(client) => {
                        *renderer = Some(client.clone());
                        tracing::info!("[Player] ✅ Connected to renderer");
                    }
                    Err(e) => {
                        tracing::debug!("[Player] ⏳ Renderer not ready: {}", e);
                        return Ok(());
                    }
                }
            }
        }
        
        // Get renderer client
        let renderer = {
            let r = self.renderer.lock().await;
            r.clone()
        };
        
        let Some(r) = renderer else { 
            return Ok(());
        };
        
        // Get input state from renderer
        let input = match r.get_input_state(Renderer::GetInputState).await {
            Ok(state) => state,
            Err(e) => {
                tracing::debug!("[Player] Failed to get input: {}", e);
                return Ok(());
            }
        };
        
        // Check for reset button
        self.handle_reset(&input).await?;
        
        // Update camera based on input
        self.update_camera(&input).await?;
        
        Ok(())
    }
}

impl PlayerService {
    async fn handle_reset(&self, input: &Renderer::InputState) -> Result<()> {
        let mut camera = self.camera.lock().await;
        let mut last_reset = self.last_reset_state.lock().await;
        
        let r_pressed = input.keys_down.contains(&KEY_R);
        let r_was_pressed = *last_reset;
        
        // Reset on edge: only when R is pressed and WASN'T pressed last frame
        if r_pressed && !r_was_pressed {
            camera.reset();
        }
        
        *last_reset = r_pressed;
        Ok(())
    }
    
    async fn update_camera(&self, input: &Renderer::InputState) -> Result<()> {
        // Get renderer client
        let renderer = {
            let r = self.renderer.lock().await;
            r.clone()
        };
        
        let Some(r) = renderer else { 
            return Ok(());
        };
        
        // Lock camera state
        let mut camera = self.camera.lock().await;
        
        // ============ MOUSE LOOK ============
        // CRITICAL FIX: Only use the delta, don't accumulate the same delta multiple times!
        let mouse_dx = input.mouse_delta[0];
        let mouse_dy = input.mouse_delta[1];
        
        if mouse_dx != 0.0 || mouse_dy != 0.0 {
            // Apply mouse delta to yaw and pitch
            camera.yaw += mouse_dx * camera.mouse_sensitivity;
            camera.pitch -= mouse_dy * camera.mouse_sensitivity;
            
            // Clamp pitch to avoid gimbal lock
            camera.pitch = camera.pitch.clamp(-PI/2.0 + 0.01, PI/2.0 - 0.01);
            
            // Normalize yaw to 0-2PI
            camera.yaw = camera.yaw.rem_euclid(2.0 * PI);
            
            // Mark that we've used the mouse
            camera.first_mouse = true;
        }
        
        // ============ MOVEMENT VECTORS ============
        // Calculate forward vector (where camera is looking, projected onto horizontal plane)
        let forward_x = camera.yaw.cos();
        let forward_z = camera.yaw.sin();
        let forward = [forward_x, 0.0, forward_z];
        
        // Calculate right vector (perpendicular to forward and up)
        let right_x = (camera.yaw + PI/2.0).cos();
        let right_z = (camera.yaw + PI/2.0).sin();
        let right = [right_x, 0.0, right_z];
        
        // ============ MOVEMENT ============
        let mut move_delta = [0.0, 0.0, 0.0];
        let speed = camera.speed * 0.016; // 16ms frame time
        
        // WASD movement (relative to camera direction)
        if input.keys_down.contains(&KEY_W) {
            move_delta[0] += forward[0] * speed;
            move_delta[2] += forward[2] * speed;
        }
        if input.keys_down.contains(&KEY_S) {
            move_delta[0] -= forward[0] * speed;
            move_delta[2] -= forward[2] * speed;
        }
        if input.keys_down.contains(&KEY_A) {
            move_delta[0] -= right[0] * speed;
            move_delta[2] -= right[2] * speed;
        }
        if input.keys_down.contains(&KEY_D) {
            move_delta[0] += right[0] * speed;
            move_delta[2] += right[2] * speed;
        }
        
        // Vertical movement (world up, not camera relative)
        if input.keys_down.contains(&KEY_SPACE) {
            move_delta[1] += speed;
        }
        if input.keys_down.contains(&KEY_LSHIFT) {
            move_delta[1] -= speed;
        }
        
        // Apply movement
        camera.position[0] += move_delta[0];
        camera.position[1] += move_delta[1];
        camera.position[2] += move_delta[2];
        
        // ============ CALCULATE TARGET ============
        // Target is position + forward vector (with pitch applied)
        let pitch_cos = camera.pitch.cos();
        let target_x = camera.position[0] + camera.yaw.cos() * pitch_cos;
        let target_y = camera.position[1] + camera.pitch.sin();
        let target_z = camera.position[2] + camera.yaw.sin() * pitch_cos;
        
        camera.target = [target_x, target_y, target_z];
        
        // ============ COPY DATA FOR RPC ============
        let position = camera.position;
        let target = camera.target;
        let up = camera.up;
        let fov = camera.fov;
        let near = camera.near;
        let far = camera.far;
        let yaw = camera.yaw.to_degrees();
        let pitch = camera.pitch.to_degrees();
        
        // Release lock
        drop(camera);
        
        // Log every 120 frames (about 2 seconds at 60fps)
        let frame = self.frame_counter.fetch_add(1, Ordering::Relaxed);
        if frame % 120 == 0 {
            tracing::info!(
                "[Player] 📷 Pos: [{:.1}, {:.1}, {:.1}] Yaw: {:.1}° Pitch: {:.1}°", 
                position[0], position[1], position[2], 
                yaw, pitch
            );
        }
        
        // Send to renderer
        match r.set_camera(Renderer::SetCamera {
            camera_id: "player".to_string(),
            position,
            target,
            up,
            fov,
            near,
            far,
        }).await {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::error!("[Player] ❌ Failed to set camera: {}", e);
                Ok(())
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("👤 Player Cell - 6DOF Camera Controller");
    println!("   ┌─────────────────────────────────────┐");
    println!("   │  Controls:                          │");
    println!("   │    W A S D  - Move forward/left/back/right │");
    println!("   │    Mouse    - Look around           │");
    println!("   │    Space    - Move up               │");
    println!("   │    Shift    - Move down             │");
    println!("   │    R        - RESET camera position │");
    println!("   └─────────────────────────────────────┘");
    println!("   └─ 🔧 FIXED: Mouse delta no longer explodes!");
    println!("   └─ ✨ ADDED: Press R to reset camera!");
    
    let service = PlayerService {
        camera: Arc::new(Mutex::new(CameraState::default())),
        renderer: Arc::new(Mutex::new(None)),
        frame_counter: Arc::new(AtomicU64::new(0)),
        last_reset_state: Arc::new(Mutex::new(false)),
    };
    
    let service_clone = service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(16));
        loop {
            interval.tick().await;
            if let Err(e) = service_clone.tick(()).await {
                tracing::error!("[Player] ❌ Tick error: {}", e);
            }
        }
    });
    
    service.serve("player").await
}