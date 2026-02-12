use std::sync::{Arc, Mutex};
use winit::window::Window;
use crate::engine::RetinaEngine;
use crate::input::Input;
use crate::InputState;

pub struct WgpuCtx {
    pub surface: wgpu::Surface<'static>,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub config: wgpu::SurfaceConfiguration,
    pub engine: RetinaEngine,
    pub window: Arc<Window>,
    pub depth_texture_view: wgpu::TextureView,
    pub input: Input,
    // Shared State for IPC
    pub shared_input: Arc<Mutex<InputState>>,
}

impl WgpuCtx {
    pub async fn new_async(window: Arc<Window>, shared_input: Arc<Mutex<InputState>>) -> Self {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.unwrap();
        
        let (device, queue) = adapter.request_device(&Default::default(), None).await.unwrap();
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let mut config = surface.get_default_config(&adapter, width, height).unwrap();

        let caps = surface.get_capabilities(&adapter);
        let mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate, wgpu::PresentMode::Fifo].into_iter()
            .find(|&m| caps.present_modes.contains(&m)).unwrap_or(wgpu::PresentMode::Fifo);
        config.present_mode = mode;
        surface.configure(&device, &config);

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            label: Some("depth"), view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&Default::default());

        let engine = RetinaEngine::ignite(device.clone(), config.format);

        Self {
            surface, device, queue, config, engine, window,
            depth_texture_view: depth_view,
            input: Input::new(),
            shared_input,
        }
    }

    pub fn resize(&mut self, new_size: (u32, u32)) {
        if new_size.0 > 0 && new_size.1 > 0 {
            self.config.width = new_size.0;
            self.config.height = new_size.1;
            self.surface.configure(&self.device, &self.config);
            let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                size: wgpu::Extent3d { width: new_size.0, height: new_size.1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                label: Some("depth"), view_formats: &[],
            });
            self.depth_texture_view = depth_texture.create_view(&Default::default());
        }
    }

    pub fn draw(&mut self) {
        // Sync Input to Shared State
        let (keys, delta) = self.input.get_and_reset_state();
        if let Ok(mut lock) = self.shared_input.lock() {
            lock.keys_down = keys;
            lock.mouse_delta[0] += delta[0];
            lock.mouse_delta[1] += delta[1];
        }

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Timeout) => return,
            Err(wgpu::SurfaceError::Outdated) => return,
            Err(wgpu::SurfaceError::Lost) => {
                self.resize((self.config.width, self.config.height));
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                eprintln!("[Renderer] Out of Memory - Exiting.");
                std::process::exit(1);
            }
        };

        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());

        // FIX: Pass &self.device as first argument
        self.engine.render(&self.device, &mut encoder, &view, &self.depth_texture_view);
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}