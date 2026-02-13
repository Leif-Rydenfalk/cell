//! World Cell - Scene Management ONLY
//!
//! This cell NO LONGER updates transforms.
//! It ONLY:
//! 1. Spawns celestial bodies once
//! 2. Provides management APIs
//! 3. Does NOT touch per-frame updates

use anyhow::Result;
use cell_sdk::*;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Duration;
use bytemuck::cast_slice;

cell_remote!(Renderer = "renderer");
cell_remote!(Orbital = "orbital");

// ========= SHADERS =========
const PLANET_SHADER: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    position: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;

var<push_constant> transform: mat4x4<f32>;

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return camera.view_proj * transform * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.2, 0.6, 1.0, 1.0);
}
"#;

const SUN_SHADER: &str = r#"
var<push_constant> transform: mat4x4<f32>;

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return transform * vec4<f32>(pos, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.8, 0.2, 1.0);
}
"#;

const CUBE_VERTICES: &[f32] = &[
    // ... (keep the same 36 vertices) ...
    -0.5, -0.5,  0.5,  0.5, -0.5,  0.5,  0.5,  0.5,  0.5,
    -0.5, -0.5,  0.5,  0.5,  0.5,  0.5, -0.5,  0.5,  0.5,
    -0.5, -0.5, -0.5, -0.5,  0.5, -0.5,  0.5,  0.5, -0.5,
    -0.5, -0.5, -0.5,  0.5,  0.5, -0.5,  0.5, -0.5, -0.5,
    -0.5, -0.5, -0.5, -0.5, -0.5,  0.5, -0.5,  0.5,  0.5,
    -0.5, -0.5, -0.5, -0.5,  0.5,  0.5, -0.5,  0.5, -0.5,
     0.5, -0.5,  0.5,  0.5, -0.5, -0.5,  0.5,  0.5, -0.5,
     0.5, -0.5,  0.5,  0.5,  0.5, -0.5,  0.5,  0.5,  0.5,
    -0.5,  0.5,  0.5,  0.5,  0.5,  0.5,  0.5,  0.5, -0.5,
    -0.5,  0.5,  0.5,  0.5,  0.5, -0.5, -0.5,  0.5, -0.5,
    -0.5, -0.5, -0.5,  0.5, -0.5, -0.5,  0.5, -0.5,  0.5,
    -0.5, -0.5, -0.5,  0.5, -0.5,  0.5, -0.5, -0.5,  0.5,
];

// ========= SERVICE =========
#[service]
#[derive(Clone)]
struct WorldService {
    renderer: Arc<Mutex<Option<Renderer::Client>>>,
    orbital: Arc<Mutex<Option<Orbital::Client>>>,
    initialized: Arc<Mutex<bool>>,
    spawned: Arc<Mutex<bool>>,
}

#[handler]
impl WorldService {
    // Tick runs at 1Hz - we only need to check if we need to respawn
    async fn tick(&self, _req: ()) -> Result<()> {
        // Connect to renderer
        let renderer = {
            let mut r = self.renderer.lock().await;
            if r.is_none() {
                if let Ok(client) = Renderer::Client::connect().await {
                    tracing::info!("[World] Connected to renderer");
                    *r = Some(client.clone());
                }
            }
            r.clone()
        };
        
        // Connect to orbital
        let orbital = {
            let mut o = self.orbital.lock().await;
            if o.is_none() {
                if let Ok(client) = Orbital::Client::connect().await {
                    tracing::info!("[World] Connected to orbital");
                    *o = Some(client.clone());
                }
            }
            o.clone()
        };
        
        let (Some(r), Some(o)) = (renderer, orbital) else {
            return Ok(());
        };
        
        // ONE-TIME initialization
        {
            let mut initialized = self.initialized.lock().await;
            if !*initialized {
                if let Err(e) = self.init_renderer(&r).await {
                    tracing::warn!("[World] Init failed: {}", e);
                    return Ok(());
                }
                *initialized = true;
            }
        }
        
        // ONE-TIME spawning
        {
            let mut spawned = self.spawned.lock().await;
            if !*spawned {
                if let Err(e) = self.spawn_bodies(&o, &r).await {
                    tracing::warn!("[World] Spawn failed: {}", e);
                    return Ok(());
                }
                *spawned = true;
                tracing::info!("[World] ✅ All bodies spawned. No further updates needed.");
            }
        }
        
        Ok(())
    }
}

impl WorldService {
    async fn init_renderer(&self, r: &Renderer::Client) -> Result<()> {
        r.register_pass(Renderer::RegisterPass {
            pass_id: "planet".to_string(),
            shader_source: PLANET_SHADER.to_string(),
            topology: "TriangleList".to_string(),
            vertex_layout: vec![
                Renderer::VertexAttribute {
                    format: "Float32x3".to_string(),
                    offset: 0,
                    shader_location: 0,
                }
            ],
        }).await?;
        
        r.register_pass(Renderer::RegisterPass {
            pass_id: "sun".to_string(),
            shader_source: SUN_SHADER.to_string(),
            topology: "TriangleList".to_string(),
            vertex_layout: vec![
                Renderer::VertexAttribute {
                    format: "Float32x3".to_string(),
                    offset: 0,
                    shader_location: 0,
                }
            ],
        }).await?;
        
        r.create_buffer(Renderer::CreateBuffer {
            buffer_id: "planet_cube".to_string(),
            size: (CUBE_VERTICES.len() * 4) as u64,
            usages: vec![
                Renderer::BufferUsage::Vertex,
                Renderer::BufferUsage::CopyDst,
            ],
        }).await?;
        
        let vertex_bytes = cast_slice(CUBE_VERTICES).to_vec();
        r.update_buffer(Renderer::UpdateBuffer {
            buffer_id: "planet_cube".to_string(),
            data: vertex_bytes,
            offset: 0,
        }).await?;
        
        tracing::info!("[World] Renderer initialized");
        Ok(())
    }
    
    async fn spawn_bodies(&self, orbital: &Orbital::Client, renderer: &Renderer::Client) -> Result<()> {
        let bodies = orbital.get_bodies(Orbital::GetBodies).await?;
        let count = bodies.bodies.len();
        
        tracing::info!("[World] Spawning {} celestial bodies...", count);
        
        for body in bodies.bodies {
            let entity_id = format!("body_{}", body.id);
            let pass_id = match body.body_type {
                Orbital::BodyType::Star => "sun".to_string(),
                _ => "planet".to_string(),
            };
            
            let scale = if body.id == "sun" {
                body.radius * 2.0
            } else {
                body.radius * 3.0
            };
            
            // Use identity transform - Orbital will push real transforms
            let transform = Self::identity_transform(scale);
            
            if let Err(e) = renderer.spawn_entity(Renderer::SpawnEntity {
                entity_id,
                pass_id,
                buffer_id: "planet_cube".to_string(),
                vertex_count: 36,
                instance_count: 1,
                transform,
            }).await {
                tracing::error!("[World] Failed to spawn {}: {}", body.name, e);
            }
        }
        
        tracing::info!("[World] ✅ Spawned {} bodies. Physics pushes transforms directly.", count);
        Ok(())
    }
    
    fn identity_transform(scale: f32) -> [f32; 16] {
        [
            scale, 0.0,   0.0,   0.0,
            0.0,   scale, 0.0,   0.0,
            0.0,   0.0,   scale, 0.0,
            0.0,   0.0,   0.0,   1.0,
        ]
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🌍 World Cell - Scene Management ONLY");
    println!("   └─ Spawns entities once");
    println!("   └─ Orbital pushes transforms directly @ 60Hz");
    println!("   └─ NO per-frame updates = NO stutter");
    
    let service = WorldService {
        renderer: Arc::new(Mutex::new(None)),
        orbital: Arc::new(Mutex::new(None)),
        initialized: Arc::new(Mutex::new(false)),
        spawned: Arc::new(Mutex::new(false)),
    };
    
    let service_clone = service.clone();
    tokio::spawn(async move {
        // Tick at 1Hz - just to detect renderer restarts
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let _ = service_clone.tick(()).await;
        }
    });
    
    service.serve("world").await
}