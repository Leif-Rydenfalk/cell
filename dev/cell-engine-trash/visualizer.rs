use std::borrow::Cow;

pub struct NodeVisualizer {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl NodeVisualizer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        shader_source: Option<&str>,
        common_header: &str,
        struct_defs: &str, // NEW: Injected struct definitions
        struct_name: &str,
        topology: wgpu::PrimitiveTopology,
    ) -> Self {
        // Header includes Common -> Struct Defs -> Bindings
        let header = format!("{}\n{}\nstruct Camera {{ view_proj: mat4x4<f32>, pos: vec3<f32>, time: f32 }};\n@group(0) @binding(0) var<storage, read> data: array<{}>;\n@group(1) @binding(0) var<uniform> camera: Camera;\n", 
            common_header, struct_defs, struct_name);

        let default_src = r#"
            struct VertexOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
            @vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOut {
                var out: VertexOut; out.pos = vec4<f32>(0.0,0.0,0.0,1.0); out.color=vec4<f32>(1.0,0.0,1.0,1.0); return out;
            }
            @fragment fn fs_main(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> { return color; }
        "#;

        let full_src = format!("{}\n{}", header, shader_source.unwrap_or(default_src));

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Vis Shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(full_src)),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Vis Data Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Global Cam Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX
                    | wgpu::ShaderStages::FRAGMENT
                    | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_layout, &cam_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Vis Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout: bind_layout,
        }
    }
}
