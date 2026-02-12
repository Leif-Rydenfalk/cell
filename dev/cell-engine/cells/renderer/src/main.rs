mod engine;
mod wgpu_ctx;
#[path = "input.rs"]
mod input;

use anyhow::Result;
use cell_sdk as cell;
use cell_sdk::membrane::Membrane;
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
    keyboard::PhysicalKey,
};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// --- 1. SHARED TYPES ---
#[cell::protein]
#[repr(u16)]
#[derive(Copy, Eq, Hash)]
pub enum KeyCode {
    Unknown = 0,
    W, A, S, D, Q, E,
    Space, Shift, Esc
}

#[cell::protein]
pub struct InputState {
    pub keys_down: Vec<u16>, 
    pub mouse_delta: [f32; 2],
}

// --- 2. THE CONTRACT ---
#[derive(Clone, Debug)]
pub enum RenderCommand {
    CreateTexture { id: String, width: u32, height: u32, format: String },
    CreateBuffer { id: String, size: u64, usage: u32 },
    RegisterPass { 
        id: String, 
        shader_source: String, 
        inputs: Vec<String>, 
        outputs: Vec<String>, 
        topology: String 
    },
    RegisterComputePass { 
        id: String, 
        shader_source: String, 
        inputs: Vec<String>, 
        outputs: Vec<String>,
        workgroups: [u32; 3] 
    },
    UpdateResource { id: String, data: Vec<u8> },
    SpawnEntity { id: String, pass_id: String, resource_id: String, vertex_count: u32 },
    DespawnEntity { id: String },
    SetCamera { position: Vec<f32>, target: Vec<f32>, up: Vec<f32> },
    GetInputState, 
}

// --- 3. SERVICE ---
#[cell::service]
struct RendererService {
    tx: mpsc::UnboundedSender<RenderCommand>,
    input_state: Arc<Mutex<InputState>>,
}

#[cell::handler]
impl RendererService {
    async fn create_texture(&self, id: String, width: u32, height: u32, format: String) -> Result<()> {
        self.tx.send(RenderCommand::CreateTexture { id, width, height, format })?;
        Ok(())
    }
    async fn create_buffer(&self, id: String, size: u64, usage: u32) -> Result<()> {
        self.tx.send(RenderCommand::CreateBuffer { id, size, usage })?;
        Ok(())
    }
    async fn register_pass(&self, id: String, shader_source: String, inputs: Vec<String>, outputs: Vec<String>, topology: String) -> Result<()> {
        self.tx.send(RenderCommand::RegisterPass { id, shader_source, inputs, outputs, topology })?;
        Ok(())
    }
    async fn register_compute_pass(&self, id: String, shader_source: String, inputs: Vec<String>, outputs: Vec<String>, workgroups: [u32; 3]) -> Result<()> {
        self.tx.send(RenderCommand::RegisterComputePass { id, shader_source, inputs, outputs, workgroups })?;
        Ok(())
    }
    async fn update_resource(&self, id: String, data: Vec<u8>) -> Result<()> {
        self.tx.send(RenderCommand::UpdateResource { id, data })?;
        Ok(())
    }
    async fn spawn_entity(&self, id: String, pass_id: String, resource_id: String, vertex_count: u32) -> Result<()> {
        self.tx.send(RenderCommand::SpawnEntity { id, pass_id, resource_id, vertex_count })?;
        Ok(())
    }
    async fn despawn_entity(&self, id: String) -> Result<()> {
        self.tx.send(RenderCommand::DespawnEntity { id })?;
        Ok(())
    }
    async fn set_camera(&self, position: Vec<f32>, target: Vec<f32>, up: Vec<f32>) -> Result<()> {
        self.tx.send(RenderCommand::SetCamera { position, target, up })?;
        Ok(())
    }
    async fn get_input_state(&self) -> Result<InputState> {
        let lock = self.input_state.lock().unwrap();
        Ok(InputState { keys_down: lock.keys_down.clone(), mouse_delta: lock.mouse_delta })
    }
}

impl Clone for RendererService {
    fn clone(&self) -> Self { Self { tx: self.tx.clone(), input_state: self.input_state.clone() } }
}

fn main() -> Result<()> {
    env_logger::init();
    
    // Create Tokio Runtime for background tasks (Membrane, Shader Compiler)
    let rt = tokio::runtime::Runtime::new().unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let shared_input = Arc::new(Mutex::new(InputState { keys_down: vec![], mouse_delta: [0.0, 0.0] }));

    let service = RendererService { tx: tx.clone(), input_state: shared_input.clone() };
    
    // Spawn Membrane task on the runtime
    rt.spawn(async move {
        let _ = Membrane::bind("renderer", move |data| {
            let mut s = service.clone(); 
            async move {
                let bytes = s.handle_cell_message(data.as_slice()).await?;
                Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
            }
        }, Some(RendererService::CELL_GENOME.to_string())).await;
    });

    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new().with_title("Cell Retina").build(&event_loop).unwrap());
    window.set_cursor_visible(false); 
    let _ = window.set_cursor_grab(winit::window::CursorGrabMode::Locked).or_else(|_| window.set_cursor_grab(winit::window::CursorGrabMode::Confined));

    // Enter runtime context so that tokio::spawn inside RetinaEngine::ignite works
    let _guard = rt.enter();
    let mut ctx = pollster::block_on(wgpu_ctx::WgpuCtx::new_async(window.clone(), shared_input));
    drop(_guard); // Release thread-local runtime context

    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);
        
        while let Ok(cmd) = rx.try_recv() {
            ctx.engine.process_command(cmd, &ctx.device, &ctx.queue, ctx.config.format, &mut ctx.input);
        }

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => ctx.resize((size.width, size.height)),
                WindowEvent::RedrawRequested => ctx.draw(),
                WindowEvent::KeyboardInput { event, .. } => {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        let key = input::map_to_u16(code);
                        if key != KeyCode::Unknown { 
                            ctx.input.handle_key_input(key, event.state);
                        }
                    }
                },
                _ => {}
            },
            Event::DeviceEvent { event: DeviceEvent::MouseMotion { delta }, .. } => ctx.input.handle_mouse_motion(delta),
            Event::AboutToWait => window.request_redraw(),
            _ => {}
        }
    }).map_err(Into::into)
}