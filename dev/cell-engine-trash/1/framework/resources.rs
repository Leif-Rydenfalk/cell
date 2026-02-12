use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub enum GpuResource {
    Buffer(Arc<wgpu::Buffer>),
    Texture(TextureBundle),
    Sampler(Arc<wgpu::Sampler>),
}

pub struct TextureBundle {
    pub texture: Arc<wgpu::Texture>,
    pub view: Arc<wgpu::TextureView>,
    pub format: wgpu::TextureFormat,
    pub size: wgpu::Extent3d,
    // Optional generic CPU buffer for async updates (Cameras, Video)
    pub cpu_source: Option<Arc<Mutex<Option<Vec<u8>>>>>,
}

pub struct ResourceManager {
    pub resources: HashMap<String, GpuResource>,
    dummy_view: Arc<wgpu::TextureView>,
}

impl ResourceManager {
    pub fn new(device: &wgpu::Device) -> Self {
        // Create a 1x1 dummy texture for binding errors
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        Self {
            resources: HashMap::new(),
            dummy_view: Arc::new(dummy.create_view(&Default::default())),
        }
    }

    pub fn clear(&mut self) {
        self.resources.clear();
    }

    // --- Adders ---

    pub fn add_buffer(&mut self, name: String, buffer: Arc<wgpu::Buffer>) {
        self.resources.insert(name, GpuResource::Buffer(buffer));
    }

    pub fn add_texture(
        &mut self,
        name: String,
        texture: Arc<wgpu::Texture>,
        view: Arc<wgpu::TextureView>,
        format: wgpu::TextureFormat,
        size: wgpu::Extent3d,
    ) {
        self.resources.insert(
            name,
            GpuResource::Texture(TextureBundle {
                texture,
                view,
                format,
                size,
                cpu_source: None,
            }),
        );
    }

    pub fn add_camera_source(&mut self, name: String, bundle: TextureBundle) {
        self.resources.insert(name, GpuResource::Texture(bundle));
    }

    pub fn add_sampler(&mut self, name: String, sampler: Arc<wgpu::Sampler>) {
        self.resources.insert(name, GpuResource::Sampler(sampler));
    }

    // --- Getters ---

    pub fn get_buffer(&self, name: &str) -> Option<&Arc<wgpu::Buffer>> {
        match self.resources.get(name) {
            Some(GpuResource::Buffer(b)) => Some(b),
            _ => None,
        }
    }

    pub fn get_texture_view(&self, name: &str) -> Option<&Arc<wgpu::TextureView>> {
        match self.resources.get(name) {
            Some(GpuResource::Texture(t)) => Some(&t.view),
            _ => None,
        }
    }

    pub fn get_texture_format(&self, name: &str) -> Option<wgpu::TextureFormat> {
        match self.resources.get(name) {
            Some(GpuResource::Texture(t)) => Some(t.format),
            _ => None,
        }
    }

    pub fn get_sampler(&self, name: &str) -> Option<&Arc<wgpu::Sampler>> {
        match self.resources.get(name) {
            Some(GpuResource::Sampler(s)) => Some(s),
            _ => None,
        }
    }

    pub fn get_dummy_view(&self) -> &Arc<wgpu::TextureView> {
        &self.dummy_view
    }

    // --- Update Logic ---

    pub fn update_sources(&self, queue: &wgpu::Queue) {
        for (_, res) in &self.resources {
            if let GpuResource::Texture(bundle) = res {
                if let Some(source) = &bundle.cpu_source {
                    let mut lock = source.lock().unwrap();
                    if let Some(data) = lock.take() {
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &bundle.texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &data,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(4 * bundle.size.width),
                                rows_per_image: Some(bundle.size.height),
                            },
                            bundle.size,
                        );
                    }
                }
            }
        }
    }
}

// Helper to spawn camera threads
pub fn spawn_camera_thread(url: String, width: u32, height: u32) -> Arc<Mutex<Option<Vec<u8>>>> {
    let buffer = Arc::new(Mutex::new(None));
    let buffer_clone = buffer.clone();

    thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(2000))
            .build()
            .unwrap_or_default();

        loop {
            if let Ok(resp) = client.get(&url).send() {
                if let Ok(bytes) = resp.bytes() {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let img =
                            img.resize_exact(width, height, image::imageops::FilterType::Nearest);
                        let rgba = img.to_rgba8();
                        let mut lock = buffer_clone.lock().unwrap();
                        *lock = Some(rgba.into_raw());
                    }
                }
            }
            thread::sleep(Duration::from_millis(33)); // ~30 FPS
        }
    });

    buffer
}
