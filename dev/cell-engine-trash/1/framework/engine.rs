use super::blueprint::{Blueprint, PassConfig, ResourceConfig};
use super::render_graph::RenderContext;
use super::resources::{spawn_camera_thread, ResourceManager, TextureBundle};
use imgui::TextureId;
use imgui_wgpu::Renderer;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlobalUniforms {
    time: f32,
    dt: f32,
    frame: u32,
    _pad1: u32,
    mouse: [f32; 4],
    screen: [f32; 2],
    _pad2: [f32; 2],
}

pub struct Engine {
    pub resource_manager: ResourceManager,
    pub config: Blueprint,

    compute_pipelines: HashMap<String, (wgpu::ComputePipeline, wgpu::BindGroup)>,
    render_pipelines: HashMap<String, (wgpu::RenderPipeline, wgpu::BindGroup)>,

    global_buffer: wgpu::Buffer,
    global_bind_group: wgpu::BindGroup,
    global_uniforms: GlobalUniforms,

    pub frame_count: u64,
    pub blueprint_path: String,

    pub imgui_texture_map: HashMap<String, TextureId>,
    pub inspector_buffer_data: Option<(String, Vec<u8>)>,
}

impl Engine {
    pub fn new(device: &wgpu::Device, blueprint_path: &str) -> Self {
        let global_uniforms = GlobalUniforms {
            time: 0.0,
            dt: 0.016,
            frame: 0,
            _pad1: 0,
            mouse: [0.0; 4],
            screen: [100.0, 100.0],
            _pad2: [0.0; 2],
        };

        let global_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Global Uniforms"),
            contents: bytemuck::cast_slice(&[global_uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Global Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::all(),
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Global BG"),
            layout: &global_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: global_buffer.as_entire_binding(),
            }],
        });

        Self {
            resource_manager: ResourceManager::new(device),
            config: Blueprint::default(),
            compute_pipelines: HashMap::new(),
            render_pipelines: HashMap::new(),
            global_buffer,
            global_bind_group,
            global_uniforms,
            frame_count: 0,
            blueprint_path: blueprint_path.to_string(),
            imgui_texture_map: HashMap::new(),
            inspector_buffer_data: None,
        }
    }

    pub fn load_graph(&mut self, device: &wgpu::Device, surface_format: wgpu::TextureFormat) {
        let json_content =
            fs::read_to_string(&self.blueprint_path).unwrap_or_else(|_| "{}".to_string());
        let config: Blueprint = serde_json::from_str(&json_content).unwrap_or_else(|e| {
            println!("Error parsing blueprint: {}", e);
            Blueprint::default()
        });
        self.load_graph_from_blueprint(device, config, surface_format);
    }

    pub fn load_graph_from_blueprint(
        &mut self,
        device: &wgpu::Device,
        config: Blueprint,
        surface_format: wgpu::TextureFormat,
    ) {
        self.config = config;
        self.rebuild_resources(device);
        self.rebuild_pipelines(device, surface_format);
    }

    pub fn register_imgui_textures(&mut self, renderer: &mut Renderer, device: &wgpu::Device) {
        for (_, id) in self.imgui_texture_map.drain() {
            renderer.textures.remove(id);
        }

        for (name, resource) in &self.resource_manager.resources {
            if let super::resources::GpuResource::Texture(bundle) = resource {
                let sampler_desc = wgpu::SamplerDescriptor {
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                };

                let config = imgui_wgpu::RawTextureConfig {
                    label: Some(name),
                    sampler_desc,
                };

                let texture_id = renderer
                    .textures
                    .insert(imgui_wgpu::Texture::from_raw_parts(
                        device,
                        renderer,
                        bundle.texture.clone(),
                        bundle.view.clone(),
                        None,
                        Some(&config),
                        bundle.size,
                    ));

                self.imgui_texture_map.insert(name.clone(), texture_id);
            }
        }
    }

    pub fn capture_buffer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        buffer_name: &str,
    ) {
        if let Some(gpu_buffer) = self.resource_manager.get_buffer(buffer_name) {
            let size = gpu_buffer.size();

            let align_mask = wgpu::COPY_BUFFER_ALIGNMENT - 1;
            let padded_size = (size + align_mask) & !align_mask;

            let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Inspector Staging"),
                size: padded_size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(gpu_buffer, 0, &staging_buffer, 0, size);
            queue.submit(Some(encoder.finish()));

            let slice = staging_buffer.slice(0..size);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());

            device.poll(wgpu::Maintain::Wait);

            if let Ok(Ok(())) = rx.recv() {
                let data = slice.get_mapped_range();
                self.inspector_buffer_data = Some((buffer_name.to_string(), data.to_vec()));
            }
            staging_buffer.unmap();
        }
    }

    fn rebuild_resources(&mut self, device: &wgpu::Device) {
        self.resource_manager.clear();
        for res in &self.config.resources {
            match res {
                ResourceConfig::Buffer { name, size, usage } => {
                    let mut usage_flags = wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::STORAGE;
                    for u in usage {
                        match u.as_str() {
                            "vertex" => usage_flags |= wgpu::BufferUsages::VERTEX,
                            "index" => usage_flags |= wgpu::BufferUsages::INDEX,
                            "uniform" => usage_flags |= wgpu::BufferUsages::UNIFORM,
                            "indirect" => usage_flags |= wgpu::BufferUsages::INDIRECT,
                            _ => {}
                        }
                    }
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(name),
                        size: *size,
                        usage: usage_flags,
                        mapped_at_creation: false,
                    });
                    self.resource_manager
                        .add_buffer(name.clone(), Arc::new(buffer));
                }
                ResourceConfig::Texture {
                    name,
                    width,
                    height,
                    format,
                } => {
                    let fmt = self.parse_format(format);
                    let size = wgpu::Extent3d {
                        width: *width,
                        height: *height,
                        depth_or_array_layers: 1,
                    };
                    let usage_flags = wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_DST;
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(name),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: fmt,
                        usage: usage_flags,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&Default::default());
                    self.resource_manager.add_texture(
                        name.clone(),
                        Arc::new(texture),
                        Arc::new(view),
                        fmt,
                        size,
                    );
                }
                ResourceConfig::Camera {
                    name,
                    url,
                    width,
                    height,
                } => {
                    let size = wgpu::Extent3d {
                        width: *width,
                        height: *height,
                        depth_or_array_layers: 1,
                    };
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(name),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&Default::default());
                    let source = spawn_camera_thread(url.clone(), *width, *height);
                    self.resource_manager.add_camera_source(
                        name.clone(),
                        TextureBundle {
                            texture: Arc::new(texture),
                            view: Arc::new(view),
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            size,
                            cpu_source: Some(source),
                        },
                    );
                }
                ResourceConfig::Image { .. } => {}
                ResourceConfig::Sampler {
                    name,
                    address_mode,
                    filter_mode,
                } => {
                    let address = match address_mode.as_str() {
                        "repeat" => wgpu::AddressMode::Repeat,
                        "mirror" => wgpu::AddressMode::MirrorRepeat,
                        _ => wgpu::AddressMode::ClampToEdge,
                    };
                    let filter = match filter_mode.as_str() {
                        "nearest" => wgpu::FilterMode::Nearest,
                        _ => wgpu::FilterMode::Linear,
                    };
                    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                        label: Some(name),
                        address_mode_u: address,
                        address_mode_v: address,
                        address_mode_w: address,
                        mag_filter: filter,
                        min_filter: filter,
                        mipmap_filter: filter,
                        ..Default::default()
                    });
                    self.resource_manager
                        .add_sampler(name.clone(), Arc::new(sampler));
                }
            }
        }
    }

    fn rebuild_pipelines(&mut self, device: &wgpu::Device, surface_format: wgpu::TextureFormat) {
        self.compute_pipelines.clear();
        self.render_pipelines.clear();

        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Global Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::all(),
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        for pass in &self.config.passes {
            match pass {
                PassConfig::Compute {
                    name,
                    shader,
                    entry_point,
                    inputs,
                    workgroups: _,
                    defines,
                    enabled,
                } => {
                    if !enabled {
                        continue;
                    }
                    let (layout, bind_group) = self.create_bindings(device, name, inputs);
                    let raw_src = fs::read_to_string(shader).unwrap_or_else(|_| "".to_string());
                    let src = self.preprocess_shader(&raw_src, defines);
                    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some(name),
                        source: wgpu::ShaderSource::Wgsl(Cow::Owned(src)),
                    });
                    let pipe_layout =
                        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                            label: Some(name),
                            bind_group_layouts: &[&layout, &global_layout],
                            push_constant_ranges: &[],
                        });
                    let pipeline =
                        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                            label: Some(name),
                            layout: Some(&pipe_layout),
                            module: &module,
                            entry_point: Some(entry_point),
                            compilation_options: Default::default(),
                            cache: None,
                        });
                    self.compute_pipelines
                        .insert(name.clone(), (pipeline, bind_group));
                }
                PassConfig::Render {
                    name,
                    shader,
                    vs_entry,
                    fs_entry,
                    inputs,
                    targets,
                    depth_target,
                    topology,
                    defines,
                    enabled,
                    vertex_count: _,
                } => {
                    if !enabled {
                        continue;
                    }
                    let (layout, bind_group) = self.create_bindings(device, name, inputs);
                    let raw_src = fs::read_to_string(shader).unwrap_or_else(|_| "".to_string());
                    let src = self.preprocess_shader(&raw_src, defines);
                    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some(name),
                        source: wgpu::ShaderSource::Wgsl(Cow::Owned(src)),
                    });
                    let pipe_layout =
                        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                            label: Some(name),
                            bind_group_layouts: &[&layout, &global_layout],
                            push_constant_ranges: &[],
                        });

                    let color_targets: Vec<Option<wgpu::ColorTargetState>> = targets
                        .iter()
                        .map(|t| {
                            let format = if t == "Backbuffer" {
                                surface_format
                            } else {
                                self.resource_manager
                                    .get_texture_format(t)
                                    .unwrap_or(wgpu::TextureFormat::Rgba8Unorm)
                            };
                            Some(wgpu::ColorTargetState {
                                format,
                                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                                write_mask: wgpu::ColorWrites::ALL,
                            })
                        })
                        .collect();

                    let depth_stencil = if let Some(depth_name) = depth_target {
                        let fmt = if depth_name == "Backbuffer" {
                            wgpu::TextureFormat::Depth32Float
                        } else {
                            self.resource_manager
                                .get_texture_format(depth_name)
                                .unwrap_or(wgpu::TextureFormat::Depth32Float)
                        };
                        Some(wgpu::DepthStencilState {
                            format: fmt,
                            depth_write_enabled: true,
                            depth_compare: wgpu::CompareFunction::Less,
                            stencil: wgpu::StencilState::default(),
                            bias: wgpu::DepthBiasState::default(),
                        })
                    } else {
                        None
                    };

                    let primitive = wgpu::PrimitiveState {
                        topology: self.parse_topology(topology),
                        ..Default::default()
                    };

                    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(name),
                        layout: Some(&pipe_layout),
                        vertex: wgpu::VertexState {
                            module: &module,
                            entry_point: Some(vs_entry),
                            buffers: &[],
                            compilation_options: Default::default(),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some(fs_entry),
                            targets: &color_targets,
                            compilation_options: Default::default(),
                        }),
                        primitive,
                        depth_stencil,
                        multisample: wgpu::MultisampleState::default(),
                        multiview: None,
                        cache: None,
                    });
                    self.render_pipelines
                        .insert(name.clone(), (pipeline, bind_group));
                }
                PassConfig::Copy { .. } => {
                    // No pipeline needed for Copy passes
                }
            }
        }
    }

    fn create_bindings(
        &self,
        device: &wgpu::Device,
        name: &str,
        inputs: &Vec<super::blueprint::BindConfig>,
    ) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
        let mut layout_entries = Vec::new();
        let mut bind_entries = Vec::new();

        for input in inputs {
            let mut resource_entry = None;
            let mut binding_type = wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            };

            if let Some(buf) = self.resource_manager.get_buffer(&input.resource) {
                binding_type = wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: !input.writable,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                };
                resource_entry = Some(wgpu::BindingResource::Buffer(
                    buf.as_entire_buffer_binding(),
                ));
            } else if let Some(view) = self.resource_manager.get_texture_view(&input.resource) {
                if input.writable {
                    binding_type = wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: self
                            .resource_manager
                            .get_texture_format(&input.resource)
                            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm),
                        view_dimension: wgpu::TextureViewDimension::D2,
                    };
                    resource_entry = Some(wgpu::BindingResource::TextureView(view));
                } else {
                    binding_type = wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    };
                    resource_entry = Some(wgpu::BindingResource::TextureView(view));
                }
            } else if let Some(sampler) = self.resource_manager.get_sampler(&input.resource) {
                binding_type = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
                resource_entry = Some(wgpu::BindingResource::Sampler(sampler));
            }

            if let Some(res) = resource_entry {
                layout_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: input.binding,
                    visibility: wgpu::ShaderStages::COMPUTE
                        | wgpu::ShaderStages::VERTEX
                        | wgpu::ShaderStages::FRAGMENT,
                    ty: binding_type,
                    count: None,
                });
                bind_entries.push(wgpu::BindGroupEntry {
                    binding: input.binding,
                    resource: res,
                });
            } else {
                println!(
                    "Warning: Resource {} not found for pass {}",
                    input.resource, name
                );
            }
        }

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{}_layout", name)),
            entries: &layout_entries,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{}_bg", name)),
            layout: &layout,
            entries: &bind_entries,
        });
        (layout, group)
    }

    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        time: f32,
        dt: f32,
        mouse: [f32; 4],
        screen: [f32; 2],
    ) {
        self.frame_count += 1;
        self.resource_manager.update_sources(queue);
        self.global_uniforms.time = time;
        self.global_uniforms.dt = dt;
        self.global_uniforms.frame = self.frame_count as u32;
        self.global_uniforms.mouse = mouse;
        self.global_uniforms.screen = screen;
        queue.write_buffer(
            &self.global_buffer,
            0,
            bytemuck::cast_slice(&[self.global_uniforms]),
        );
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        backbuffer_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
    ) {
        let dummy_view = self.resource_manager.get_dummy_view();
        let _context = RenderContext {
            device,
            queue,
            encoder,
            resources: &self.resource_manager,
            backbuffer_view,
            depth_view,
        };

        // 1. Handle Copy Passes (Ping-Pong logic)
        // We do this first or interleave it, but simplest is to just run passes in order
        // However, RenderPass and ComputePass scopes in wgpu are restrictive.
        // Copy commands must happen *outside* of a Pass.
        // So we must iterate passes and break out of scopes.

        // Note: In this implementation, we iterate sequentially.
        // This breaks batching slightly but ensures correct ordering.

        for pass in &self.config.passes {
            match pass {
                PassConfig::Compute {
                    name,
                    workgroups,
                    enabled,
                    ..
                } => {
                    if *enabled {
                        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some(name),
                            timestamp_writes: None,
                        });
                        cpass.set_bind_group(1, &self.global_bind_group, &[]);
                        if let Some((pipeline, bind_group)) = self.compute_pipelines.get(name) {
                            cpass.set_pipeline(pipeline);
                            cpass.set_bind_group(0, bind_group, &[]);
                            cpass.dispatch_workgroups(workgroups[0], workgroups[1], workgroups[2]);
                        }
                    }
                }
                PassConfig::Render {
                    name,
                    targets,
                    depth_target,
                    vertex_count,
                    enabled,
                    ..
                } => {
                    if !*enabled {
                        continue;
                    }

                    let color_attachments_views: Vec<&wgpu::TextureView> = targets
                        .iter()
                        .map(|t| {
                            if t == "Backbuffer" {
                                backbuffer_view
                            } else {
                                self.resource_manager
                                    .get_texture_view(t)
                                    .map(|v| v.as_ref())
                                    .unwrap_or_else(|| dummy_view.as_ref())
                            }
                        })
                        .collect();

                    let depth_attachment_view = depth_target.as_ref().map(|dt| {
                        if dt == "Backbuffer" {
                            depth_view
                        } else {
                            self.resource_manager
                                .get_texture_view(dt)
                                .map(|v| v.as_ref())
                                .unwrap_or_else(|| dummy_view.as_ref())
                        }
                    });

                    let color_attachments: Vec<Option<wgpu::RenderPassColorAttachment>> =
                        color_attachments_views
                            .iter()
                            .map(|view| {
                                Some(wgpu::RenderPassColorAttachment {
                                    view,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                        store: wgpu::StoreOp::Store,
                                    },
                                })
                            })
                            .collect();

                    let depth_stencil =
                        depth_attachment_view.map(|view| wgpu::RenderPassDepthStencilAttachment {
                            view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        });

                    {
                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some(name),
                            color_attachments: &color_attachments,
                            depth_stencil_attachment: depth_stencil,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });

                        if let Some((pipeline, bind_group)) = self.render_pipelines.get(name) {
                            rpass.set_pipeline(pipeline);
                            rpass.set_bind_group(0, bind_group, &[]);
                            rpass.set_bind_group(1, &self.global_bind_group, &[]);
                            rpass.draw(0..*vertex_count, 0..1);
                        }
                    }
                }
                PassConfig::Copy {
                    name: _,
                    source,
                    destination,
                    enabled,
                } => {
                    if *enabled {
                        if let (Some(src), Some(dst)) = (
                            self.resource_manager.get_buffer(source),
                            self.resource_manager.get_buffer(destination),
                        ) {
                            let size = src.size();
                            if size == dst.size() {
                                encoder.copy_buffer_to_buffer(src, 0, dst, 0, size);
                            } else {
                                eprintln!(
                                    "Copy Error: Size mismatch between {} and {}",
                                    source, destination
                                );
                            }
                        } else {
                            eprintln!(
                                "Copy Error: Buffers {} or {} not found",
                                source, destination
                            );
                        }
                    }
                }
            }
        }
    }

    fn preprocess_shader(&self, source: &str, defines: &[String]) -> String {
        let mut header = String::new();
        for def in defines {
            header.push_str(&format!("// Define Active: {}\n", def));
        }
        let mut processed = source.to_string();
        for def in defines {
            processed = processed.replace(
                &format!("// #ifdef {}", def),
                &format!("// ENABLED {}", def),
            );
            processed = processed.replace(
                &format!("const ENABLE_{} = false;", def),
                &format!("const ENABLE_{} = true;", def),
            );
        }
        format!("{}\n{}", header, processed)
    }

    fn parse_format(&self, fmt: &str) -> wgpu::TextureFormat {
        match fmt {
            "rgba8unorm" => wgpu::TextureFormat::Rgba8Unorm,
            "rgba32float" => wgpu::TextureFormat::Rgba32Float,
            "bgra8unorm" => wgpu::TextureFormat::Bgra8Unorm,
            "depth32float" => wgpu::TextureFormat::Depth32Float,
            "r32float" => wgpu::TextureFormat::R32Float,
            "r32uint" => wgpu::TextureFormat::R32Uint,
            _ => wgpu::TextureFormat::Rgba8Unorm,
        }
    }

    fn parse_topology(&self, topo: &str) -> wgpu::PrimitiveTopology {
        match topo {
            "PointList" => wgpu::PrimitiveTopology::PointList,
            "LineList" => wgpu::PrimitiveTopology::LineList,
            _ => wgpu::PrimitiveTopology::TriangleList,
        }
    }
}
