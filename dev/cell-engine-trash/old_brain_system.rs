use crate::{vertex, CameraFeed, ShaderHotReload};
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;
use wgpu::util::DeviceExt;

const NEURON_COUNT: u32 = 200_000;
const GRID_DIM: u32 = 512;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Neuron {
    pub semantic: [u32; 4], // 16 bytes
    pub pos: [f32; 2],      // 8 bytes
    pub voltage: f32,       // 4 bytes (Activation/Error)
    pub prediction: f32,    // 4 bytes (Memoized state)
    pub precision: f32,     // 4 bytes (Plasticity factor)
    pub layer: u32,         // 4 bytes (0..6)
    pub fatigue: f32,       // 4 bytes Boost factor
    pub boredom: f32,       // 4 bytes Padding
} // Total 48 bytes

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SimParams {
    pub neuron_count: u32,
    pub time: f32,
    pub dt: f32,
    pub grid_dim: u32,
    pub train_mode: u32,
    pub use_camera: u32,
    pub _pad0: f32,
    pub _pad1: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct LineVertex {
    pos: [f32; 4],
    color: [f32; 4],
}

pub struct BrainSystem {
    queue: Arc<wgpu::Queue>,
    neuron_buffer: wgpu::Buffer,
    param_buffer: wgpu::Buffer,
    spatial_grid_buffer: wgpu::Buffer,
    line_buffer: wgpu::Buffer,
    vertex_buffer_cube: wgpu::Buffer,
    index_buffer_cube: wgpu::Buffer,
    pub input_texture: Arc<wgpu::Texture>,
    pub input_texture_view: Arc<wgpu::TextureView>,
    pub output_texture: Arc<wgpu::Texture>,
    pub output_texture_view: Arc<wgpu::TextureView>,
    pub prediction_texture: Arc<wgpu::Texture>,
    pub prediction_texture_view: Arc<wgpu::TextureView>,
    pub camera: CameraFeed,

    // Split compute bind groups for read/write prediction texture conflict
    compute_bg_write: wgpu::BindGroup,
    compute_bg_read: wgpu::BindGroup,

    render_points_bind_group: wgpu::BindGroup,
    empty_bind_group: wgpu::BindGroup,
    init_pipeline: wgpu::ComputePipeline,
    clear_grid_pipeline: wgpu::ComputePipeline,
    populate_grid_pipeline: wgpu::ComputePipeline,
    update_neurons_pipeline: wgpu::ComputePipeline,
    generate_lines_pipeline: wgpu::ComputePipeline,
    render_error_pipeline: wgpu::ComputePipeline,
    render_dream_pipeline: wgpu::ComputePipeline,
    render_pipeline_points: wgpu::RenderPipeline,
    render_pipeline_lines: wgpu::RenderPipeline,
    pub params: SimParams,
    start_time: std::time::Instant,
    initialized: bool,
}

impl BrainSystem {
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        shader_hot_reload: &ShaderHotReload,
        camera_url: String,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let params = SimParams {
            neuron_count: NEURON_COUNT,
            time: 0.0,
            dt: 0.016,
            grid_dim: GRID_DIM,
            train_mode: 1,
            use_camera: 1,
            _pad0: 0.0,
            _pad1: 0.0,
        };

        let neuron_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Neuron Buffer"),
            size: (std::mem::size_of::<Neuron>() * NEURON_COUNT as usize) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        let param_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sim Params"),
            contents: bytemuck::cast_slice(&[params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let spatial_grid_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Spatial Grid"),
            size: (GRID_DIM as u64 * GRID_DIM as u64 * 4 * 8),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let line_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Line Buffer"),
            size: (NEURON_COUNT as u64 * 2 * std::mem::size_of::<LineVertex>() as u64),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        let vertex_buffer_cube = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Cube Vertices"),
            contents: bytemuck::cast_slice(vertex::VERTICES_CUBE),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let mut cube_indices: Vec<u16> = Vec::new();
        for i in 0..6 {
            let base = (i * 4) as u16;
            cube_indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }

        let index_buffer_cube = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Cube Indices"),
            contents: bytemuck::cast_slice(&cube_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let tex_desc = wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 512,
                height: 512,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };

        let input_texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Input"),
            ..tex_desc
        }));
        let input_texture_view = Arc::new(input_texture.create_view(&Default::default()));

        let output_texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Output"),
            ..tex_desc
        }));
        let output_texture_view = Arc::new(output_texture.create_view(&Default::default()));

        let prediction_texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Pred"),
            ..tex_desc
        }));
        let prediction_texture_view = Arc::new(prediction_texture.create_view(&Default::default()));

        // Dummy texture for split bind groups
        let void_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Void Tex"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let void_view = void_texture.create_view(&Default::default());

        let input_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Brain Compute Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let entries_common = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: neuron_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: param_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: spatial_grid_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: line_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&input_texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&input_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&output_texture_view),
            },
        ];

        // Group 1: WRITE to Prediction (Read slot is dummy)
        let mut entries_write = entries_common.clone();
        entries_write.push(wgpu::BindGroupEntry {
            binding: 7,
            resource: wgpu::BindingResource::TextureView(&prediction_texture_view),
        });
        entries_write.push(wgpu::BindGroupEntry {
            binding: 8,
            resource: wgpu::BindingResource::TextureView(&void_view),
        });

        let compute_bg_write = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Brain BG Write"),
            layout: &compute_layout,
            entries: &entries_write,
        });

        // Group 2: READ from Prediction (Write slot is dummy)
        let mut entries_read = entries_common.clone();
        entries_read.push(wgpu::BindGroupEntry {
            binding: 7,
            resource: wgpu::BindingResource::TextureView(&void_view),
        });
        entries_read.push(wgpu::BindGroupEntry {
            binding: 8,
            resource: wgpu::BindingResource::TextureView(&prediction_texture_view),
        });

        let compute_bg_read = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Brain BG Read"),
            layout: &compute_layout,
            entries: &entries_read,
        });

        let render_points_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Points Layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let render_points_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Points BG"),
            layout: &render_points_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: neuron_buffer.as_entire_binding(),
            }],
        });

        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        let empty_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &empty_layout,
            entries: &[],
        });

        let shader = shader_hot_reload.get_shader("brain.wgsl");
        let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&compute_layout],
            push_constant_ranges: &[],
        });

        let mk_compute = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipe_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };

        let init_pipeline = mk_compute("cs_init_neurons");
        let clear_grid_pipeline = mk_compute("cs_clear_grid");
        let populate_grid_pipeline = mk_compute("cs_populate_grid");
        let update_neurons_pipeline = mk_compute("cs_update_neurons");
        let generate_lines_pipeline = mk_compute("cs_generate_lines");
        let render_error_pipeline = mk_compute("cs_render_error");
        let render_dream_pipeline = mk_compute("cs_render_dream");

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&render_points_layout, camera_bind_group_layout],
                push_constant_ranges: &[],
            });

        let render_pipeline_points =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Points Pipe"),
                layout: Some(&render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[vertex::create_vertex_buffer_layout()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });

        let line_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&empty_layout, camera_bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline_lines =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Line Pipe"),
                layout: Some(&line_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_lines"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 32,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 16,
                                shader_location: 1,
                            },
                        ],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_lines"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });

        Self {
            queue,
            neuron_buffer,
            param_buffer,
            spatial_grid_buffer,
            line_buffer,
            vertex_buffer_cube,
            index_buffer_cube,
            input_texture,
            input_texture_view,
            output_texture,
            output_texture_view,
            prediction_texture,
            prediction_texture_view,
            camera: CameraFeed::new(camera_url),
            compute_bg_write,
            compute_bg_read,
            render_points_bind_group,
            empty_bind_group,
            init_pipeline,
            clear_grid_pipeline,
            populate_grid_pipeline,
            update_neurons_pipeline,
            generate_lines_pipeline,
            render_error_pipeline,
            render_dream_pipeline,
            render_pipeline_points,
            render_pipeline_lines,
            params,
            start_time: std::time::Instant::now(),
            initialized: false,
        }
    }

    pub fn update(&mut self, input: &crate::input::Input, _window_size: (u32, u32)) {
        self.params.time = self.start_time.elapsed().as_secs_f32();

        if input.is_key_pressed(winit::keyboard::KeyCode::Space) {
            self.params.train_mode ^= 1;
        }
        if input.is_key_pressed(winit::keyboard::KeyCode::KeyC) {
            self.params.use_camera ^= 1;
        }

        if self.params.use_camera == 1 {
            if let Some(img) = self.camera.get_frame() {
                let img =
                    image::imageops::resize(&img, 512, 512, image::imageops::FilterType::Nearest);
                let data: Vec<f32> = img.as_raw().iter().map(|&b| b as f32 / 255.0).collect();
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.input_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&data),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(512 * 16),
                        rows_per_image: Some(512),
                    },
                    wgpu::Extent3d {
                        width: 512,
                        height: 512,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        self.queue
            .write_buffer(&self.param_buffer, 0, bytemuck::cast_slice(&[self.params]));
    }

    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        camera_bg: &wgpu::BindGroup,
        depth_view: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        // --- COMPUTE PASS 1: Update neurons and populate grid ---
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Compute Phase 1"),
                timestamp_writes: None,
            });

            cpass.set_bind_group(0, &self.compute_bg_write, &[]);

            if !self.initialized {
                cpass.set_pipeline(&self.init_pipeline);
                cpass.dispatch_workgroups((NEURON_COUNT + 63) / 64, 1, 1);
                self.initialized = true;
            }

            cpass.set_pipeline(&self.clear_grid_pipeline);
            cpass.dispatch_workgroups((GRID_DIM * GRID_DIM + 63) / 64, 1, 1);

            cpass.set_pipeline(&self.populate_grid_pipeline);
            cpass.dispatch_workgroups((NEURON_COUNT + 63) / 64, 1, 1);

            // FIRST: Generate prediction from current state
            cpass.set_pipeline(&self.render_dream_pipeline);
            cpass.dispatch_workgroups(64, 64, 1);
        }

        // Copy prediction to input if dreaming
        if self.params.use_camera == 0 {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.prediction_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.input_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: 512,
                    height: 512,
                    depth_or_array_layers: 1,
                },
            );
        }

        // --- COMPUTE PASS 2: Update neurons based on prediction error ---
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Compute Phase 2"),
                timestamp_writes: None,
            });

            cpass.set_bind_group(0, &self.compute_bg_read, &[]);

            // Update neurons (this computes error in L0)
            cpass.set_pipeline(&self.update_neurons_pipeline);
            cpass.dispatch_workgroups((NEURON_COUNT + 63) / 64, 1, 1);

            // NOW render the error visualization
            cpass.set_pipeline(&self.render_error_pipeline);
            cpass.dispatch_workgroups(64, 64, 1);

            cpass.set_pipeline(&self.generate_lines_pipeline);
            cpass.dispatch_workgroups((NEURON_COUNT + 63) / 64, 1, 1);
        }

        // Render
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(&self.render_pipeline_lines);
            rpass.set_bind_group(0, &self.empty_bind_group, &[]);
            rpass.set_bind_group(1, camera_bg, &[]);
            rpass.set_vertex_buffer(0, self.line_buffer.slice(..));
            rpass.draw(0..(NEURON_COUNT * 2), 0..1);
            rpass.set_pipeline(&self.render_pipeline_points);
            rpass.set_bind_group(0, &self.render_points_bind_group, &[]);
            rpass.set_bind_group(1, camera_bg, &[]);
            rpass.set_vertex_buffer(0, self.vertex_buffer_cube.slice(..));
            rpass.set_index_buffer(self.index_buffer_cube.slice(..), wgpu::IndexFormat::Uint16);
            rpass.draw_indexed(0..36, 0, 0..NEURON_COUNT);
        }
    }

    pub fn get_textures(
        &self,
    ) -> (
        Arc<wgpu::Texture>,
        Arc<wgpu::TextureView>,
        Arc<wgpu::Texture>,
        Arc<wgpu::TextureView>,
        Arc<wgpu::Texture>,
        Arc<wgpu::TextureView>,
    ) {
        (
            self.input_texture.clone(),
            self.input_texture_view.clone(),
            self.prediction_texture.clone(),
            self.prediction_texture_view.clone(),
            self.output_texture.clone(),
            self.output_texture_view.clone(),
        )
    }
}
