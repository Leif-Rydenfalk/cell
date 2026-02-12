use crate::RenderCommand;
use crate::input::Input;
use cell_sdk::vesicle::Vesicle;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use tokio::sync::mpsc;

// --- RESOURCE MANAGEMENT ---
enum Resource {
    Buffer(wgpu::Buffer),
    Texture(Arc<wgpu::Texture>, Arc<wgpu::TextureView>),
}

// --- PASS DEFINITIONS ---
enum PipelineType {
    Render(Arc<wgpu::RenderPipeline>),
    Compute(Arc<wgpu::ComputePipeline>),
}

struct PassNode {
    id: String,
    pipeline: PipelineType,
    inputs: Vec<String>,  // Resources to read (BindGroup entries)
    outputs: Vec<String>, // Resources to write (RenderTargets or Storage Write)
    // Render specific
    topology: Option<wgpu::PrimitiveTopology>,
    // Compute specific
    workgroups: Option<[u32; 3]>,
}

struct ActiveEntity {
    pass_id: String,
    resource_id: String,
    vertex_count: u32,
}

pub struct RetinaEngine {
    resources: HashMap<String, Resource>,
    passes: HashMap<String, PassNode>,
    entities: HashMap<String, ActiveEntity>,
    
    // Execution Order (Rebuilt when passes change)
    execution_order: Vec<String>,
    dirty_graph: bool,

    // Async Compiler
    fallback_pipeline: Arc<wgpu::RenderPipeline>,
    compiler_tx: mpsc::UnboundedSender<(String, String, bool, Option<[u32;3]>)>, 
    compiler_rx: mpsc::UnboundedReceiver<(String, Result<PipelineType, String>)>,
}

impl RetinaEngine {
    pub fn ignite(device: Arc<wgpu::Device>, format: wgpu::TextureFormat) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(String, String, bool, Option<[u32; 3]>)>();
        let (res_tx, res_rx) = mpsc::unbounded_channel::<(String, Result<PipelineType, String>)>();

        // Basic Error Pipeline
        let error_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Error"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("error.wgsl")))
        });
        
        let fallback = Arc::new(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Fallback"), layout: None,
            vertex: wgpu::VertexState { module: &error_mod, entry_point: "vs_main", buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &error_mod, entry_point: "fs_main", targets: &[Some(format.into())], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState::default(), 
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(), bias: wgpu::DepthBiasState::default()
            }), 
            multisample: wgpu::MultisampleState::default(), multiview: None,
        }));

        // --- ASYNC SHADER COMPILER ---
        let dev = device.clone();
        tokio::spawn(async move {
            while let Some((id, src, is_compute, workgroups)) = rx.recv().await {
                dev.push_error_scope(wgpu::ErrorFilter::Validation);
                
                let module = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&id), source: wgpu::ShaderSource::Wgsl(Cow::Owned(src))
                });
                
                let layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(&format!("{}_layout", id)),
                    bind_group_layouts: &[], // Auto-layout
                    push_constant_ranges: &[],
                });

                // FIX: Explicitly type result to avoid inference error E0282
                let result: Result<PipelineType, ()> = if is_compute {
                    let pipeline = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some(&id), layout: Some(&layout), module: &module, entry_point: "main",
                        compilation_options: Default::default(),
                               });
                    Ok(PipelineType::Compute(Arc::new(pipeline)))
                } else {
                    let pipeline = dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(&id), layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: &module, entry_point: "vs_main",
                            buffers: &[wgpu::VertexBufferLayout {
                                array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: &[wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                                              wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 }]
                            }],
                            compilation_options: Default::default(),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &module, entry_point: "fs_main",
                            targets: &[Some(format.into())],
                            compilation_options: Default::default(),
                        }),
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: Some(wgpu::DepthStencilState {
                            format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less,
                            stencil: wgpu::StencilState::default(), bias: wgpu::DepthBiasState::default()
                        }),
                        multisample: wgpu::MultisampleState::default(), multiview: None,
                 
                    });
                    Ok(PipelineType::Render(Arc::new(pipeline)))
                };

                if let Some(e) = dev.pop_error_scope().await {
                    let _ = res_tx.send((id, Err(e.to_string())));
                } else {
                    if let Ok(p) = result { let _ = res_tx.send((id, Ok(p))); }
                }
            }
        });

        Self {
            resources: HashMap::new(), passes: HashMap::new(), entities: HashMap::new(),
            execution_order: Vec::new(), dirty_graph: false,
            fallback_pipeline: fallback, compiler_tx: tx, compiler_rx: res_rx,
        }
    }

    pub fn process_command(&mut self, cmd: RenderCommand, device: &wgpu::Device, queue: &wgpu::Queue, _format: wgpu::TextureFormat, _input: &mut Input) -> Option<Vesicle> {
        // Check for compiled shaders
        while let Ok((id, res)) = self.compiler_rx.try_recv() {
            match res {
                Ok(pipe) => { if let Some(pass) = self.passes.get_mut(&id) { pass.pipeline = pipe; } }
                Err(e) => eprintln!("[Retina] Shader '{}' Error: {}", id, e),
            }
        }

        match cmd {
            RenderCommand::CreateTexture { id, width, height, format } => {
                 let fmt = match format.as_str() {
                     "depth32" => wgpu::TextureFormat::Depth32Float,
                     "rgba32float" => wgpu::TextureFormat::Rgba32Float,
                     _ => wgpu::TextureFormat::Rgba8Unorm
                 };
                 let tex = device.create_texture(&wgpu::TextureDescriptor {
                     label: Some(&id), size: wgpu::Extent3d{width,height,depth_or_array_layers:1}, 
                     mip_level_count:1, sample_count:1, dimension:wgpu::TextureDimension::D2, format:fmt, 
                     usage: wgpu::TextureUsages::all(), 
                     view_formats:&[]
                 });
                 let view = tex.create_view(&Default::default());
                 self.resources.insert(id, Resource::Texture(Arc::new(tex), Arc::new(view)));
            }
            RenderCommand::CreateBuffer { id, size, usage } => {
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&id), size, usage: wgpu::BufferUsages::from_bits_truncate(usage), mapped_at_creation: false
                });
                self.resources.insert(id, Resource::Buffer(buf));
            }
            RenderCommand::RegisterPass { id, shader_source, inputs, outputs, topology } => {
                let _ = self.compiler_tx.send((id.clone(), shader_source, false, None));
                // FIX: Use topology variable or store it
                let prim_topology = match topology.as_str() {
                    "LineList" => wgpu::PrimitiveTopology::LineList,
                    "PointList" => wgpu::PrimitiveTopology::PointList,
                    _ => wgpu::PrimitiveTopology::TriangleList,
                };
                self.passes.insert(id.clone(), PassNode {
                    id, pipeline: PipelineType::Render(self.fallback_pipeline.clone()),
                    inputs, outputs, topology: Some(prim_topology), workgroups: None
                });
                self.dirty_graph = true;
            }
            RenderCommand::RegisterComputePass { id, shader_source, inputs, outputs, workgroups } => {
                let _ = self.compiler_tx.send((id.clone(), shader_source, true, Some(workgroups)));
                self.passes.insert(id.clone(), PassNode {
                    id, pipeline: PipelineType::Render(self.fallback_pipeline.clone()), 
                    inputs, outputs, topology: None, workgroups: Some(workgroups)
                });
                self.dirty_graph = true;
            }
            RenderCommand::UpdateResource { id, data } => {
                if let Some(res) = self.resources.get(&id) {
                    match res {
                        Resource::Buffer(buf) => queue.write_buffer(buf, 0, &data),
                        Resource::Texture(tex, _) => {
                            let size = tex.size();
                            queue.write_texture(
                                wgpu::ImageCopyTexture { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                                &data,
                                wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * size.width), rows_per_image: Some(size.height) },
                                size,
                            );
                        }
                    }
                } else {
                    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label:Some(&id), contents:&data, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST });
                    self.resources.insert(id, Resource::Buffer(buf));
                }
            }
            RenderCommand::SpawnEntity { id, pass_id, resource_id, vertex_count } => {
                self.entities.insert(id, ActiveEntity { pass_id, resource_id, vertex_count });
            }
            RenderCommand::DespawnEntity { id } => { self.entities.remove(&id); }
            _ => {}
        }
        None
    }

    fn rebuild_graph(&mut self) {
        if !self.dirty_graph { return; }
        
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        
        for pass_id in self.passes.keys() {
            in_degree.insert(pass_id.clone(), 0);
        }

        // Build dependency graph: writer -> reader
        for (reader_id, reader) in &self.passes {
            for input_res in &reader.inputs {
                for (writer_id, writer) in &self.passes {
                    if writer_id == reader_id { continue; }
                    if writer.outputs.contains(input_res) {
                        adj.entry(writer_id.clone()).or_default().push(reader_id.clone());
                        *in_degree.get_mut(reader_id).unwrap() += 1;
                    }
                }
            }
        }

        let mut queue = Vec::new();
        for (id, &deg) in &in_degree {
            if deg == 0 { queue.push(id.clone()); }
        }
        
        queue.sort(); // Deterministic order

        self.execution_order.clear();
        while let Some(u) = queue.pop() {
            self.execution_order.push(u.clone());
            if let Some(neighbors) = adj.get(&u) {
                for v in neighbors {
                    let deg = in_degree.get_mut(v).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(v.clone());
                    }
                }
            }
        }
        
        // Handle cycles/orphans
        for id in self.passes.keys() {
            if !self.execution_order.contains(id) {
                self.execution_order.push(id.clone());
            }
        }

        self.dirty_graph = false;
        println!("[Retina] Render Graph Rebuilt: {:?}", self.execution_order);
    }

    pub fn render(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, screen: &wgpu::TextureView, depth: &wgpu::TextureView) {
        self.rebuild_graph();

        // Always Clear
        {
             let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: screen, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color{r:0.02, g:0.02, b:0.05, a:1.0}), store: wgpu::StoreOp::Store }
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth, depth_ops: Some(wgpu::Operations{load:wgpu::LoadOp::Clear(1.0), store:wgpu::StoreOp::Store}), stencil_ops: None
                }),
                timestamp_writes: None, occlusion_query_set: None,
            });
        }

        for pass_id in &self.execution_order {
            if let Some(pass) = self.passes.get(pass_id) {
                
                // Build BindGroups dynamically
                let mut entries = Vec::new();
                for (i, res_id) in pass.inputs.iter().enumerate() {
                    if let Some(res) = self.resources.get(res_id) {
                        let resource = match res {
                            Resource::Buffer(b) => b.as_entire_binding(),
                            Resource::Texture(_, v) => wgpu::BindingResource::TextureView(v),
                        };
                        entries.push(wgpu::BindGroupEntry { binding: i as u32, resource });
                    }
                }
                
                let bg = match &pass.pipeline {
                    PipelineType::Render(p) => {
                        if !entries.is_empty() {
                            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some(&format!("{}_bg", pass_id)),
                                layout: &p.get_bind_group_layout(0),
                                entries: &entries,
                            }))
                        } else { None }
                    },
                    PipelineType::Compute(p) => {
                        if !entries.is_empty() {
                            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some(&format!("{}_bg", pass_id)),
                                layout: &p.get_bind_group_layout(0),
                                entries: &entries,
                            }))
                        } else { None }
                    }
                };

                match &pass.pipeline {
                    PipelineType::Compute(pipeline) => {
                        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(pass_id), timestamp_writes: None });
                        cpass.set_pipeline(pipeline);
                        if let Some(bg) = &bg { cpass.set_bind_group(0, bg, &[]); }
                        if let Some(wg) = pass.workgroups {
                            cpass.dispatch_workgroups(wg[0], wg[1], wg[2]);
                        }
                    },
                    PipelineType::Render(pipeline) => {
                        let target_view = if let Some(tex_id) = pass.outputs.first() {
                             if let Some(Resource::Texture(_, v)) = self.resources.get(tex_id) { v } else { screen }
                        } else { screen };

                        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some(pass_id),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: target_view, resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } 
                            })],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: depth, depth_ops: Some(wgpu::Operations{load:wgpu::LoadOp::Load, store:wgpu::StoreOp::Store}), stencil_ops: None
                            }),
                            timestamp_writes: None, occlusion_query_set: None,
                        });
                        
                        rpass.set_pipeline(pipeline);
                        if let Some(bg) = &bg { rpass.set_bind_group(0, bg, &[]); }
                        
                        for entity in self.entities.values() {
                            if entity.pass_id == *pass_id {
                                if let Some(Resource::Buffer(buf)) = self.resources.get(&entity.resource_id) {
                                    rpass.set_vertex_buffer(0, buf.slice(..));
                                    rpass.draw(0..entity.vertex_count, 0..1);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}