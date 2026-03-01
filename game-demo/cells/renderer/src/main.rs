//! Retina Cell - Production Renderer Service
//!
//! This cell:
//! 1. Owns the wgpu device/queue and window
//! 2. Provides a stable RPC API for graphics operations
//! 3. 60 FPS render loop with WORKING camera and transforms
//! 4. TYPED input API - semantic movement controls!

use anyhow::Result;
use cell_sdk::*;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use wgpu::util::DeviceExt;
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use winit::keyboard::{KeyCode, PhysicalKey};
use std::time::Instant;
use cgmath::{Matrix4, SquareMatrix};

// ========= PROTEINS (PUBLIC API) =========
#[protein]
pub struct RegisterPass {
    pub pass_id: String,
    pub shader_source: String,
    pub topology: String,
    pub vertex_layout: Vec<VertexAttribute>,
}

#[protein]
pub struct VertexAttribute {
    pub format: String,
    pub offset: u64,
    pub shader_location: u32,
}

#[protein]
pub struct CreateBuffer {
    pub buffer_id: String,
    pub size: u64,
    pub usages: Vec<BufferUsage>,
}

#[protein]
pub enum BufferUsage {
    Vertex,
    Index,
    Uniform,
    Storage,
    Indirect,
    CopySrc,
    CopyDst,
    MapRead,
    MapWrite,
}

impl BufferUsage {
    pub fn to_wgpu_bits(&self) -> u32 {
        match self {
            BufferUsage::Vertex => wgpu::BufferUsages::VERTEX.bits(),
            BufferUsage::Index => wgpu::BufferUsages::INDEX.bits(),
            BufferUsage::Uniform => wgpu::BufferUsages::UNIFORM.bits(),
            BufferUsage::Storage => wgpu::BufferUsages::STORAGE.bits(),
            BufferUsage::Indirect => wgpu::BufferUsages::INDIRECT.bits(),
            BufferUsage::CopySrc => wgpu::BufferUsages::COPY_SRC.bits(),
            BufferUsage::CopyDst => wgpu::BufferUsages::COPY_DST.bits(),
            BufferUsage::MapRead => wgpu::BufferUsages::MAP_READ.bits(),
            BufferUsage::MapWrite => wgpu::BufferUsages::MAP_WRITE.bits(),
        }
    }
}

#[protein]
pub struct UpdateBuffer {
    pub buffer_id: String,
    pub data: Vec<u8>,
    pub offset: u64,
}

#[protein]
pub struct SpawnEntity {
    pub entity_id: String,
    pub pass_id: String,
    pub buffer_id: String,
    pub vertex_count: u32,
    pub instance_count: u32,
    pub transform: [f32; 16],
}

#[protein]
pub struct DespawnEntity {
    pub entity_id: String,
}

#[protein]
pub struct BatchUpdateTransforms {
    pub updates: Vec<TransformUpdate>,
}

#[protein]
pub struct TransformUpdate {
    pub entity_id: String,
    pub transform: [f32; 16],
}

#[protein]
pub struct UpdateTransform {
    pub entity_id: String,
    pub transform: [f32; 16],
}

#[protein]
pub struct SetCamera {
    pub camera_id: String,
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub up: [f32; 3],
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

// ========= TYPED INPUT API =========
// NO MORE RAW KEY CODES - semantic input only!
#[protein]
pub struct GetInputState;

#[protein]
#[derive(Default)]
pub struct InputState {
    // Movement controls - semantic, not hardware-specific
    pub move_forward: bool,   // W or Up Arrow
    pub move_backward: bool,  // S or Down Arrow
    pub move_left: bool,      // A or Left Arrow
    pub move_right: bool,     // D or Right Arrow
    pub move_up: bool,        // Space
    pub move_down: bool,      // Shift or Ctrl
    
    // Camera control
    pub look_delta: [f32; 2], // Mouse delta (accumulated, zeroed after read)
    
    // Actions (edge-triggered - true only on press frame)
    pub reset: bool,          // R pressed this frame
    pub escape: bool,         // Escape pressed this frame
    
    // Raw access for debugging or advanced use
    pub keys_down: Vec<String>,
    pub mouse_position: [f32; 2],
    pub mouse_buttons: u8,
    pub scroll_delta: f32,
}

#[protein]
pub struct Ping;
#[protein]
pub struct Pong {
    pub timestamp: u64,
}

// ========= INTERNAL STATE =========
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    position: [f32; 4],
}

struct RenderPass {
    pipeline: wgpu::RenderPipeline,
    topology: wgpu::PrimitiveTopology,
    bind_group: wgpu::BindGroup,
}

struct RenderEntity {
    pass_id: String,
    buffer_id: String,
    vertex_count: u32,
    instance_count: u32,
    transform: [f32; 16],
}

#[derive(Clone)]
struct Camera {
    position: [f32; 3],
    target: [f32; 3],
    up: [f32; 3],
    fov: f32,
    near: f32,
    far: f32,
    view_matrix: [f32; 16],
    proj_matrix: [f32; 16],
}

struct RendererState {
    passes: HashMap<String, RenderPass>,
    buffers: HashMap<String, Arc<wgpu::Buffer>>,
    entities: HashMap<String, RenderEntity>,
    cameras: HashMap<String, Camera>,
    active_camera: String,
    frame_count: u64,
    last_frame_time: Instant,
    camera_uniform: CameraUniform,
    camera_buffer: wgpu::Buffer,
    camera_bind_group_layout: wgpu::BindGroupLayout,
}

// ========= INPUT PROCESSING =========
struct InputAccumulator {
    // Semantic state
    move_forward: bool,
    move_backward: bool,
    move_left: bool,
    move_right: bool,
    move_up: bool,
    move_down: bool,
    
    // Mouse
    look_delta: [f32; 2],
    mouse_position: [f32; 2],
    mouse_buttons: u8,
    scroll_delta: f32,
    
    // Edge-triggered actions (cleared after read)
    reset_pressed: bool,
    escape_pressed: bool,
    
    // Raw key tracking
    keys_down: Vec<String>,
}

impl Default for InputAccumulator {
    fn default() -> Self {
        Self {
            move_forward: false,
            move_backward: false,
            move_left: false,
            move_right: false,
            move_up: false,
            move_down: false,
            look_delta: [0.0, 0.0],
            mouse_position: [0.0, 0.0],
            mouse_buttons: 0,
            scroll_delta: 0.0,
            reset_pressed: false,
            escape_pressed: false,
            keys_down: Vec::new(),
        }
    }
}

impl InputAccumulator {
    fn process_key(&mut self, key: &str, pressed: bool) {
        if pressed {
            if !self.keys_down.contains(&key.to_string()) {
                self.keys_down.push(key.to_string());
            }
        } else {
            self.keys_down.retain(|k| k != key);
        }
        
        // Map to semantic controls
        match key {
            "KeyW" | "ArrowUp" => self.move_forward = pressed,
            "KeyS" | "ArrowDown" => self.move_backward = pressed,
            "KeyA" | "ArrowLeft" => self.move_left = pressed,
            "KeyD" | "ArrowRight" => self.move_right = pressed,
            "Space" => self.move_up = pressed,
            "ShiftLeft" | "ShiftRight" | "ControlLeft" | "ControlRight" => {
                self.move_down = pressed;
            }
            "KeyR" => {
                if pressed {
                    self.reset_pressed = true;
                }
            }
            "Escape" => {
                if pressed {
                    self.escape_pressed = true;
                }
            }
            _ => {}
        }
    }
    
    fn consume(&mut self) -> InputState {
        InputState {
            move_forward: self.move_forward,
            move_backward: self.move_backward,
            move_left: self.move_left,
            move_right: self.move_right,
            move_up: self.move_up,
            move_down: self.move_down,
            look_delta: self.look_delta,
            mouse_position: self.mouse_position,
            mouse_buttons: self.mouse_buttons,
            scroll_delta: self.scroll_delta,
            reset: std::mem::take(&mut self.reset_pressed),
            escape: std::mem::take(&mut self.escape_pressed),
            keys_down: self.keys_down.clone(),
        }
    }
    
    fn reset_frame(&mut self) {
        self.look_delta = [0.0, 0.0];
        self.scroll_delta = 0.0;
    }
}

enum InputEvent {
    KeyPress(String),
    KeyRelease(String),
    MouseMove(f32, f32),
    MouseButton(u8, bool),
    Scroll(f32),
}

// ========= RENDERER SERVICE =========
#[service]
#[derive(Clone)]
struct RendererService {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: Arc<wgpu::SurfaceConfiguration>,
    surface: Arc<wgpu::Surface<'static>>,
    depth_view: Arc<wgpu::TextureView>,
    state: Arc<RwLock<RendererState>>,
    input_accumulator: Arc<parking_lot::Mutex<InputAccumulator>>,
    input_events: Arc<parking_lot::Mutex<Vec<InputEvent>>>,
}

#[handler]
impl RendererService {
    async fn ping(&self, _req: Ping) -> Result<Pong> {
        Ok(Pong {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        })
    }
    
    async fn register_pass(&self, req: RegisterPass) -> Result<()> {
        let topology = match req.topology.as_str() {
            "LineList" => wgpu::PrimitiveTopology::LineList,
            "LineStrip" => wgpu::PrimitiveTopology::LineStrip,
            "PointList" => wgpu::PrimitiveTopology::PointList,
            _ => wgpu::PrimitiveTopology::TriangleList,
        };
        
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&req.pass_id),
            source: wgpu::ShaderSource::Wgsl(req.shader_source.into()),
        });
        
        let mut attributes = Vec::new();
        let mut stride = 0;
        for attr in &req.vertex_layout {
            let format = match attr.format.as_str() {
                "Float32x2" => wgpu::VertexFormat::Float32x2,
                "Float32x3" => wgpu::VertexFormat::Float32x3,
                "Float32x4" => wgpu::VertexFormat::Float32x4,
                "Unorm8x4" => wgpu::VertexFormat::Unorm8x4,
                _ => wgpu::VertexFormat::Float32x3,
            };
            attributes.push(wgpu::VertexAttribute {
                format,
                offset: attr.offset,
                shader_location: attr.shader_location,
            });
            stride += match format {
                wgpu::VertexFormat::Float32x2 => 8,
                wgpu::VertexFormat::Float32x3 => 12,
                wgpu::VertexFormat::Float32x4 => 16,
                wgpu::VertexFormat::Unorm8x4 => 4,
                _ => 12,
            };
        }
        
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        };
        
        let state = self.state.read();
        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(&format!("{}_layout", req.pass_id)),
            bind_group_layouts: &[&state.camera_bind_group_layout],
            push_constant_ranges: &[wgpu::PushConstantRange {
                stages: wgpu::ShaderStages::VERTEX,
                range: 0..64,
            }],
        });
        drop(state);
        
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&req.pass_id),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[vertex_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        
        let mut state = self.state.write();
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{}_camera_bg", req.pass_id)),
            layout: &state.camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: state.camera_buffer.as_entire_binding(),
            }],
        });
        
        state.passes.insert(req.pass_id.clone(), RenderPass {
            pipeline,
            topology,
            bind_group,
        });
        
        tracing::info!("[Renderer] Registered pass: {}", req.pass_id);
        Ok(())
    }

    async fn batch_update_transforms(&self, req: BatchUpdateTransforms) -> Result<()> {
        let mut state = self.state.write();
        for update in req.updates {
            if let Some(entity) = state.entities.get_mut(&update.entity_id) {
                entity.transform = update.transform;
            }
        }
        Ok(())
    }
    
    async fn create_buffer(&self, req: CreateBuffer) -> Result<()> {
        let mut usage_bits = 0;
        for usage in req.usages {
            usage_bits |= usage.to_wgpu_bits();
        }
        
        let usage = wgpu::BufferUsages::from_bits_truncate(usage_bits)
            | wgpu::BufferUsages::COPY_DST;
        
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&req.buffer_id),
            size: req.size,
            usage,
            mapped_at_creation: false,
        });
        
        let mut state = self.state.write();
        state.buffers.insert(req.buffer_id, Arc::new(buffer));
        Ok(())
    }
    
    async fn update_buffer(&self, req: UpdateBuffer) -> Result<()> {
        let state = self.state.read();
        if let Some(buffer) = state.buffers.get(&req.buffer_id) {
            self.queue.write_buffer(buffer, req.offset, &req.data);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Buffer not found: {}", req.buffer_id))
        }
    }
    
    async fn spawn_entity(&self, req: SpawnEntity) -> Result<()> {
        let mut state = self.state.write();
        
        if !state.passes.contains_key(&req.pass_id) {
            return Err(anyhow::anyhow!("Pass not found: {}", req.pass_id));
        }
        if !state.buffers.contains_key(&req.buffer_id) {
            return Err(anyhow::anyhow!("Buffer not found: {}", req.buffer_id));
        }
        
        state.entities.insert(req.entity_id.clone(), RenderEntity {
            pass_id: req.pass_id,
            buffer_id: req.buffer_id,
            vertex_count: req.vertex_count,
            instance_count: req.instance_count,
            transform: req.transform,
        });
        
        tracing::info!("[Renderer] ✅ Spawned entity: {}", req.entity_id);
        Ok(())
    }
    
    async fn despawn_entity(&self, req: DespawnEntity) -> Result<()> {
        let mut state = self.state.write();
        state.entities.remove(&req.entity_id);
        Ok(())
    }
    
    async fn update_transform(&self, req: UpdateTransform) -> Result<()> {
        let mut state = self.state.write();
        if let Some(entity) = state.entities.get_mut(&req.entity_id) {
            entity.transform = req.transform;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Entity not found: {}", req.entity_id))
        }
    }
    
    async fn set_camera(&self, req: SetCamera) -> Result<()> {
        let mut state = self.state.write();
        let camera_id = req.camera_id.clone();
        
        // Calculate view matrix
        let eye = cgmath::Point3::new(req.position[0], req.position[1], req.position[2]);
        let target = cgmath::Point3::new(req.target[0], req.target[1], req.target[2]);
        let up = cgmath::Vector3::new(req.up[0], req.up[1], req.up[2]);
        
        let view = cgmath::Matrix4::look_at_rh(eye, target, up);
        let proj = cgmath::perspective(
            cgmath::Deg(req.fov),
            self.config.width as f32 / self.config.height as f32,
            req.near,
            req.far,
        );
        
        let view_matrix: [[f32; 4]; 4] = view.into();
        let proj_matrix: [[f32; 4]; 4] = proj.into();
        
        let view_flat = [
            view_matrix[0][0], view_matrix[0][1], view_matrix[0][2], view_matrix[0][3],
            view_matrix[1][0], view_matrix[1][1], view_matrix[1][2], view_matrix[1][3],
            view_matrix[2][0], view_matrix[2][1], view_matrix[2][2], view_matrix[2][3],
            view_matrix[3][0], view_matrix[3][1], view_matrix[3][2], view_matrix[3][3],
        ];
        
        let proj_flat = [
            proj_matrix[0][0], proj_matrix[0][1], proj_matrix[0][2], proj_matrix[0][3],
            proj_matrix[1][0], proj_matrix[1][1], proj_matrix[1][2], proj_matrix[1][3],
            proj_matrix[2][0], proj_matrix[2][1], proj_matrix[2][2], proj_matrix[2][3],
            proj_matrix[3][0], proj_matrix[3][1], proj_matrix[3][2], proj_matrix[3][3],
        ];
        
        state.cameras.insert(camera_id.clone(), Camera {
            position: req.position,
            target: req.target,
            up: req.up,
            view_matrix: view_flat,
            proj_matrix: proj_flat,
            fov: req.fov,
            near: req.near,
            far: req.far,
        });
        
        state.active_camera = camera_id.clone();
        
        tracing::info!("[Renderer] 📷 Active camera set to '{}'", state.active_camera);
        Ok(())
    }
    
    async fn get_input_state(&self, _req: GetInputState) -> Result<InputState> {
        // Process all pending events
        let mut events = self.input_events.lock();
        let mut accumulator = self.input_accumulator.lock();
        
        for event in events.drain(..) {
            match event {
                InputEvent::KeyPress(key) => accumulator.process_key(&key, true),
                InputEvent::KeyRelease(key) => accumulator.process_key(&key, false),
                InputEvent::MouseMove(dx, dy) => {
                    accumulator.look_delta[0] += dx;
                    accumulator.look_delta[1] += dy;
                    accumulator.mouse_position[0] += dx;
                    accumulator.mouse_position[1] += dy;
                }
                InputEvent::MouseButton(btn, pressed) => {
                    if pressed {
                        accumulator.mouse_buttons |= btn;
                    } else {
                        accumulator.mouse_buttons &= !btn;
                    }
                }
                InputEvent::Scroll(dy) => {
                    accumulator.scroll_delta += dy;
                }
            }
        }
        
        // Consume state for this frame
        let mut state = accumulator.consume();
        
        // Reset per-frame accumulators
        accumulator.reset_frame();
        
        // Also ensure mouse position is bounded to window
        state.mouse_position[0] = state.mouse_position[0].clamp(0.0, self.config.width as f32);
        state.mouse_position[1] = state.mouse_position[1].clamp(0.0, self.config.height as f32);
        
        Ok(state)
    }
}

impl RendererService {
    fn update_camera_uniform(&self) {
        let (camera_data, active_camera_name) = {
            let state = self.state.read();
            
            if let Some(cam) = state.cameras.get(&state.active_camera) {
                (cam.clone(), state.active_camera.clone())
            } else {
                drop(state);
                
                // Create default camera
                let eye = cgmath::Point3::new(20.0, 15.0, 40.0);
                let target = cgmath::Point3::new(0.0, 0.0, 0.0);
                let up = cgmath::Vector3::new(0.0, 1.0, 0.0);
                
                let view = cgmath::Matrix4::look_at_rh(eye, target, up);
                let proj = cgmath::perspective(
                    cgmath::Deg(60.0),
                    self.config.width as f32 / self.config.height as f32,
                    0.1,
                    1000.0,
                );
                
                let view_matrix: [[f32; 4]; 4] = view.into();
                let proj_matrix: [[f32; 4]; 4] = proj.into();
                
                let view_flat = [
                    view_matrix[0][0], view_matrix[0][1], view_matrix[0][2], view_matrix[0][3],
                    view_matrix[1][0], view_matrix[1][1], view_matrix[1][2], view_matrix[1][3],
                    view_matrix[2][0], view_matrix[2][1], view_matrix[2][2], view_matrix[2][3],
                    view_matrix[3][0], view_matrix[3][1], view_matrix[3][2], view_matrix[3][3],
                ];
                
                let proj_flat = [
                    proj_matrix[0][0], proj_matrix[0][1], proj_matrix[0][2], proj_matrix[0][3],
                    proj_matrix[1][0], proj_matrix[1][1], proj_matrix[1][2], proj_matrix[1][3],
                    proj_matrix[2][0], proj_matrix[2][1], proj_matrix[2][2], proj_matrix[2][3],
                    proj_matrix[3][0], proj_matrix[3][1], proj_matrix[3][2], proj_matrix[3][3],
                ];
                
                let mut state = self.state.write();
                state.cameras.insert("default".to_string(), Camera {
                    position: [20.0, 15.0, 40.0],
                    target: [0.0, 0.0, 0.0],
                    up: [0.0, 1.0, 0.0],
                    view_matrix: view_flat,
                    proj_matrix: proj_flat,
                    fov: 60.0,
                    near: 0.1,
                    far: 1000.0,
                });
                
                if state.active_camera.is_empty() {
                    state.active_camera = "default".to_string();
                }
                
                let cam = state.cameras.get(&state.active_camera).unwrap().clone();
                (cam, state.active_camera.clone())
            }
        };
        
        // Calculate view-projection matrix
        let view = Matrix4::new(
            camera_data.view_matrix[0], camera_data.view_matrix[1], camera_data.view_matrix[2], camera_data.view_matrix[3],
            camera_data.view_matrix[4], camera_data.view_matrix[5], camera_data.view_matrix[6], camera_data.view_matrix[7],
            camera_data.view_matrix[8], camera_data.view_matrix[9], camera_data.view_matrix[10], camera_data.view_matrix[11],
            camera_data.view_matrix[12], camera_data.view_matrix[13], camera_data.view_matrix[14], camera_data.view_matrix[15],
        );
        
        let proj = Matrix4::new(
            camera_data.proj_matrix[0], camera_data.proj_matrix[1], camera_data.proj_matrix[2], camera_data.proj_matrix[3],
            camera_data.proj_matrix[4], camera_data.proj_matrix[5], camera_data.proj_matrix[6], camera_data.proj_matrix[7],
            camera_data.proj_matrix[8], camera_data.proj_matrix[9], camera_data.proj_matrix[10], camera_data.proj_matrix[11],
            camera_data.proj_matrix[12], camera_data.proj_matrix[13], camera_data.proj_matrix[14], camera_data.proj_matrix[15],
        );
        
        let view_proj = proj * view;
        let view_proj_array: [[f32; 4]; 4] = view_proj.into();
        
        // Update uniform buffer
        let mut state = self.state.write();
        state.camera_uniform = CameraUniform {
            view_proj: view_proj_array,
            position: [camera_data.position[0], camera_data.position[1], camera_data.position[2], 1.0],
        };
        
        self.queue.write_buffer(
            &state.camera_buffer,
            0,
            bytemuck::bytes_of(&state.camera_uniform),
        );
    }
    
    fn render_frame(&self) {
        self.update_camera_uniform();
        
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Surface error: {:?}", e);
                return;
            }
        };
        
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        
        // Clear
        {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.02, g: 0.02, b: 0.05, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        
        // Draw entities
        let state = self.state.read();
        let entity_count = state.entities.len();
        
        if entity_count > 0 {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            
            for entity in state.entities.values() {
                if let Some(pass) = state.passes.get(&entity.pass_id) {
                    if let Some(buffer) = state.buffers.get(&entity.buffer_id) {
                        rpass.set_pipeline(&pass.pipeline);
                        rpass.set_bind_group(0, &pass.bind_group, &[]);
                        rpass.set_push_constants(
                            wgpu::ShaderStages::VERTEX,
                            0,
                            bytemuck::cast_slice(&entity.transform),
                        );
                        rpass.set_vertex_buffer(0, buffer.slice(..));
                        rpass.draw(0..entity.vertex_count, 0..entity.instance_count);
                    }
                }
            }
        }
        
        drop(state);
        
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        
        let mut state = self.state.write();
        state.frame_count += 1;
        state.last_frame_time = Instant::now();
        
        if state.frame_count % 6000 == 0 {
            tracing::info!("[Renderer] Frame {}, entities: {}", state.frame_count, entity_count);
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();

    println!("🎨 Renderer Cell - TYPED INPUT API");
    println!("   └─ 🔧 Semantic movement controls (move_forward, move_left, etc)");
    println!("   └─ 🔧 Edge-triggered actions (reset, escape)");
    println!("   └─ 🔧 No more raw key codes!");
    println!("   └─ 🔧 Push constants for transforms");
    println!("   └─ 🔧 Camera matrices in shaders");

    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new()
        .with_title("Cell Game Engine - Renderer")
        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
        .build(&event_loop)
        .unwrap());

    let instance = wgpu::Instance::default();
    let surface = instance.create_surface(window.clone()).unwrap();
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }).await.unwrap();
    
    let (device, queue) = adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::PUSH_CONSTANTS,
            required_limits: wgpu::Limits {
                max_push_constant_size: 128,
                ..Default::default()
            },
        }, 
        None
    ).await.unwrap();
    
    let device = Arc::new(device);
    let queue = Arc::new(queue);

    let size = window.inner_size();
    let config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).unwrap();
    surface.configure(&device, &config);
    let config = Arc::new(config);
    let surface = Arc::new(surface);

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = Arc::new(depth_texture.create_view(&Default::default()));

    // Create camera bind group layout
    let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("camera_bind_group_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    // Create camera uniform buffer
    let camera_uniform = CameraUniform {
        view_proj: Matrix4::identity().into(),
        position: [20.0, 15.0, 40.0, 1.0],
    };
    
    let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("camera_uniform"),
        contents: bytemuck::bytes_of(&camera_uniform),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });

    let service = RendererService {
        device: device.clone(),
        queue: queue.clone(),
        config: config.clone(),
        surface: surface.clone(),
        depth_view: depth_view.clone(),
        state: Arc::new(RwLock::new(RendererState {
            passes: HashMap::new(),
            buffers: HashMap::new(),
            entities: HashMap::new(),
            cameras: HashMap::new(),
            active_camera: "".to_string(),
            frame_count: 0,
            last_frame_time: Instant::now(),
            camera_uniform,
            camera_buffer,
            camera_bind_group_layout,
        })),
        input_accumulator: Arc::new(parking_lot::Mutex::new(InputAccumulator::default())),
        input_events: Arc::new(parking_lot::Mutex::new(Vec::new())),
    };
    
    let service_clone = service.clone();
    let input_events = service.input_events.clone();
    
    tokio::spawn(async move {
        tracing::info!("[Renderer] Starting RPC server on 'renderer'");
        if let Err(e) = service_clone.serve("renderer").await {
            tracing::error!("[Renderer] RPC server failed: {}", e);
        }
    });

    tracing::info!("[Renderer] Ready. Waiting for client connections...");

    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);
        
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(size) => {
                    let mut config = (*service.config).clone();
                    config.width = size.width.max(1);
                    config.height = size.height.max(1);
                    service.surface.configure(&service.device, &config);
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        let key_str = match code {
                            KeyCode::KeyW => "KeyW",
                            KeyCode::KeyA => "KeyA",
                            KeyCode::KeyS => "KeyS",
                            KeyCode::KeyD => "KeyD",
                            KeyCode::KeyR => "KeyR",
                            KeyCode::Space => "Space",
                            KeyCode::ShiftLeft => "ShiftLeft",
                            KeyCode::ShiftRight => "ShiftRight",
                            KeyCode::ControlLeft => "ControlLeft",
                            KeyCode::ControlRight => "ControlRight",
                            KeyCode::Escape => "Escape",
                            KeyCode::ArrowUp => "ArrowUp",
                            KeyCode::ArrowDown => "ArrowDown",
                            KeyCode::ArrowLeft => "ArrowLeft",
                            KeyCode::ArrowRight => "ArrowRight",
                            _ => return,
                        }.to_string();
                        
                        let mut events = input_events.lock();
                        match event.state {
                            ElementState::Pressed => {
                                events.push(InputEvent::KeyPress(key_str));
                            }
                            ElementState::Released => {
                                events.push(InputEvent::KeyRelease(key_str));
                            }
                        }
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let mut events = input_events.lock();
                    events.push(InputEvent::MouseMove(position.x as f32, position.y as f32));
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let btn = match button {
                        MouseButton::Left => 1,
                        MouseButton::Right => 2,
                        MouseButton::Middle => 4,
                        _ => 0,
                    };
                    let mut events = input_events.lock();
                    events.push(InputEvent::MouseButton(btn, state == ElementState::Pressed));
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
                    };
                    let mut events = input_events.lock();
                    events.push(InputEvent::Scroll(dy));
                }
                _ => {}
            },
            Event::DeviceEvent { event, .. } => {
                if let DeviceEvent::MouseMotion { delta } = event {
                    let mut events = input_events.lock();
                    events.push(InputEvent::MouseMove(delta.0 as f32, delta.1 as f32));
                }
            }
            Event::AboutToWait => {
                service.render_frame();
                window.request_redraw();
            }
            _ => {}
        }
    }).map_err(Into::into)
}