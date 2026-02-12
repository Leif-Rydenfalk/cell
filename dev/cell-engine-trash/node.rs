use super::blueprint::NodeConfig;
use super::common::COMMON_WGSL;
use super::resources::ResourceManager;
use super::visualizer::NodeVisualizer;
use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use wgpu::util::DeviceExt;

pub struct BrainNode {
    pub config: NodeConfig,
    pub pipeline: wgpu::ComputePipeline,
    pub bind_group_a: wgpu::BindGroup,
    pub bind_group_b: Option<wgpu::BindGroup>,

    pub buffer_a: Arc<wgpu::Buffer>,
    pub buffer_b: Option<Arc<wgpu::Buffer>>,
    pub param_buffer: wgpu::Buffer,
    pub staging_buffer: wgpu::Buffer,

    pub params: Vec<f32>,
    pub last_modified: SystemTime,
    pub error_msg: Option<String>,

    pub visualizer: Option<NodeVisualizer>,
    pub viz_bg_a: Option<wgpu::BindGroup>,
    pub viz_bg_b: Option<wgpu::BindGroup>,
}

impl BrainNode {
    pub fn new(
        device: &wgpu::Device,
        resources: &ResourceManager,
        config: NodeConfig,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let path = PathBuf::from(&config.shader_path);
        let total_size = config.struct_size_bytes * config.count as u64;

        // 1. Allocator
        let buffer_a: Arc<wgpu::Buffer>;
        let mut buffer_b: Option<Arc<wgpu::Buffer>> = None;

        if let Some(ext_name) = &config.external_memory {
            // Shared Memory
            buffer_a = resources
                .get_buffer(ext_name)
                .unwrap_or_else(|| {
                    panic!("Node {}: Shared buffer '{}' not found", config.id, ext_name)
                })
                .clone();
        } else {
            // Local Memory
            let save_path = format!("saves/{}.bin", config.id);
            let initial_data =
                fs::read(&save_path).unwrap_or_else(|_| vec![0u8; total_size as usize]);

            let fallback_data = vec![0u8; total_size as usize];
            let content_slice = if initial_data.len() as u64 == total_size {
                &initial_data
            } else {
                &fallback_data
            };

            buffer_a = Arc::new(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("{} Buf A", config.id)),
                    contents: content_slice,
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::VERTEX,
                }),
            );

            if config.use_ping_pong {
                buffer_b = Some(Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("{} Buf B", config.id)),
                    size: total_size,
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::COPY_DST
                        | wgpu::BufferUsages::VERTEX,
                    mapped_at_creation: false,
                })));
            }
        }

        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging"),
            size: total_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut params = vec![0.0f32; config.param_count];
        while params.len() % 4 != 0 {
            params.push(0.0);
        }

        let param_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // 2. Read Source & Compile
        let raw_code = fs::read_to_string(&path).unwrap_or_else(|_| "fn main() {}".to_string());
        let (pipeline, layout, err) = Self::compile(device, &raw_code, &config);

        // 3. Bind Groups
        let create_bg = |buf_read: &wgpu::Buffer, buf_write: &wgpu::Buffer| {
            let mut entries = Vec::new();
            let mut b_idx = 0;

            // 0: Params
            entries.push(wgpu::BindGroupEntry {
                binding: b_idx,
                resource: param_buffer.as_entire_binding(),
            });
            b_idx += 1;

            // 1: Read Buffer (neurons)
            entries.push(wgpu::BindGroupEntry {
                binding: b_idx,
                resource: buf_read.as_entire_binding(),
            });
            b_idx += 1;

            // 2: Write Buffer (neurons_out) - ONLY if ping-pong
            if config.use_ping_pong {
                entries.push(wgpu::BindGroupEntry {
                    binding: b_idx,
                    resource: buf_write.as_entire_binding(),
                });
                b_idx += 1;
            }

            // Shared buffers
            for acc in &config.access_buffers {
                if let Some(b) = resources.get_buffer(&acc.name) {
                    entries.push(wgpu::BindGroupEntry {
                        binding: b_idx,
                        resource: b.as_entire_binding(),
                    });
                } else {
                    eprintln!(
                        "Warning: Node {} missing shared buffer {}",
                        config.id, acc.name
                    );
                }
                b_idx += 1;
            }

            // Textures
            for name in config
                .input_textures
                .iter()
                .chain(&config.target_textures)
                .chain(&config.output_textures)
            {
                let view = resources
                    .get_view(name)
                    .unwrap_or(resources.get_dummy_view());
                entries.push(wgpu::BindGroupEntry {
                    binding: b_idx,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                b_idx += 1;
            }

            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{} BG", config.id)),
                layout: &layout,
                entries: &entries,
            })
        };

        let bg_a;
        let bg_b;

        if config.use_ping_pong {
            // Copy initial data to buffer_b so both buffers start with the same state
            if let Some(buf_b) = &buffer_b {
                let mut init_encoder = device.create_command_encoder(&Default::default());
                init_encoder.copy_buffer_to_buffer(&buffer_a, 0, buf_b, 0, total_size);
                // This will be submitted by the caller
            }

            // Bind Group A: Read from B, Write to A
            bg_a = create_bg(buffer_b.as_deref().unwrap(), &buffer_a);
            // Bind Group B: Read from A, Write to B
            bg_b = Some(create_bg(&buffer_a, buffer_b.as_deref().unwrap()));
        } else {
            bg_a = create_bg(&buffer_a, &buffer_a);
            bg_b = None;
        }

        // 4. Visualizer
        let mut visualizer = None;
        let mut viz_bg_a = None;
        let mut viz_bg_b = None;
        if config.visualize {
            let vis_path = config
                .visualizer_shader_path
                .as_deref()
                .unwrap_or("src/shaders/vis/neuron_vis.wgsl");
            let code = fs::read_to_string(vis_path).ok();

            // EXTRACT STRUCTS
            let struct_defs = Self::preprocess(&raw_code, &[]);

            let v = NodeVisualizer::new(
                device,
                surface_format,
                code.as_deref(),
                COMMON_WGSL,
                &struct_defs,
                &config.struct_name,
                config.wgpu_topology(),
            );

            let mk_vbg = |b: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Viz BG"),
                    layout: &v.bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: b.as_entire_binding(),
                    }],
                })
            };

            viz_bg_a = Some(mk_vbg(&buffer_a));
            if let Some(b) = &buffer_b {
                viz_bg_b = Some(mk_vbg(b));
            }
            visualizer = Some(v);
        }

        Self {
            config,
            pipeline,
            bind_group_a: bg_a,
            bind_group_b: bg_b,
            buffer_a,
            buffer_b,
            param_buffer,
            staging_buffer,
            params,
            last_modified: SystemTime::now(),
            error_msg: err,
            visualizer,
            viz_bg_a,
            viz_bg_b,
        }
    }

    fn preprocess(code: &str, defines: &[String]) -> String {
        let mut processed = String::new();
        let mut active_stack = vec![true];

        for line in code.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("// #ifdef ") {
                let def = trimmed.trim_start_matches("// #ifdef ").trim();
                let is_defined = defines.contains(&def.to_string());
                active_stack.push(*active_stack.last().unwrap() && is_defined);
                continue;
            }
            if trimmed.starts_with("// #endif") {
                if active_stack.len() > 1 {
                    active_stack.pop();
                }
                continue;
            }
            if *active_stack.last().unwrap() {
                processed.push_str(line);
                processed.push('\n');
            }
        }
        processed
    }

    fn compile(
        device: &wgpu::Device,
        raw_code: &str,
        config: &NodeConfig,
    ) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout, Option<String>) {
        let processed_code = Self::preprocess(raw_code, &config.defines);

        let mut header = String::from(COMMON_WGSL);
        let mut idx = 0;

        header.push_str(&format!(
            "@group(0) @binding({}) var<uniform> params: {};\n",
            idx, config.param_struct_name
        ));
        idx += 1;

        let access = if config.use_ping_pong {
            "read"
        } else {
            "read_write"
        };
        header.push_str(&format!(
            "@group(0) @binding({}) var<storage, {}> neurons: array<{}>;\n",
            idx, access, config.struct_name
        ));
        idx += 1;

        if config.use_ping_pong {
            header.push_str(&format!(
                "@group(0) @binding({}) var<storage, read_write> neurons_out: array<{}>;\n",
                idx, config.struct_name
            ));
            idx += 1;
        }

        for acc in &config.access_buffers {
            let a = if acc.writable { "read_write" } else { "read" };
            // FIX: Use shader_name alias if provided to avoid "neurons" collision
            let var_name = acc.shader_name.as_ref().unwrap_or(&acc.name).to_lowercase();
            header.push_str(&format!(
                "@group(0) @binding({}) var<storage, {}> {}: array<{}>;\n",
                idx, a, var_name, acc.type_name
            ));
            idx += 1;
        }

        for t in &config.input_textures {
            header.push_str(&format!(
                "@group(0) @binding({}) var {}: texture_2d<f32>;\n",
                idx,
                t.to_lowercase()
            ));
            idx += 1;
        }
        for t in &config.target_textures {
            header.push_str(&format!(
                "@group(0) @binding({}) var {}: texture_2d<f32>;\n",
                idx,
                t.to_lowercase()
            ));
            idx += 1;
        }
        for t in &config.output_textures {
            header.push_str(&format!(
                "@group(0) @binding({}) var {}: texture_storage_2d<rgba32float, write>;\n",
                idx,
                t.to_lowercase()
            ));
            idx += 1;
        }

        header.push_str("@group(1) @binding(0) var<uniform> global: GlobalParams;\n");

        let full_src = format!("{}\n{}", header, processed_code);

        // Layout Creation
        let mut entries = Vec::new();
        let mut l_idx = 0;

        entries.push(wgpu::BindGroupLayoutEntry {
            binding: l_idx,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        l_idx += 1;
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: l_idx,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage {
                    read_only: config.use_ping_pong,
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        l_idx += 1;
        if config.use_ping_pong {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: l_idx,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
            l_idx += 1;
        }

        for acc in &config.access_buffers {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: l_idx,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: !acc.writable,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
            l_idx += 1;
        }

        let tex_count = config.input_textures.len() + config.target_textures.len();
        for _ in 0..tex_count {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: l_idx,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            l_idx += 1;
        }
        for _ in &config.output_textures {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: l_idx,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba32Float,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            });
            l_idx += 1;
        }

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&config.id),
            entries: &entries,
        });
        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Global"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE
                    | wgpu::ShaderStages::VERTEX
                    | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&config.id),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(full_src)),
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(&config.id),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: &[&layout, &global_layout],
                    push_constant_ranges: &[],
                }),
            ),
            module: &module,
            entry_point: Some(&config.entry_point),
            compilation_options: Default::default(),
            cache: None,
        });

        (pipeline, layout, None)
    }

    pub fn update_params(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.param_buffer, 0, bytemuck::cast_slice(&self.params));
    }

    pub fn request_save(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_buffer_to_buffer(
            &self.buffer_a,
            0,
            &self.staging_buffer,
            0,
            self.staging_buffer.size(),
        );
    }
}
