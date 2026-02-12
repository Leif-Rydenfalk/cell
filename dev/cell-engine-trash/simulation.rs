use super::blueprint::{Blueprint, NodeConfig};
use super::inputs::InputNode;
use super::node::BrainNode;
use super::render_graph::{BufferHandle, RenderGraph, TextureHandle};
use super::resources::ResourceManager;
use imgui::*;
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
    mouse_btn: u32,
    mouse_pos: [f32; 2],
    screen_size: [f32; 2],
}

pub struct Simulation {
    pub nodes: Vec<BrainNode>,
    pub input_nodes: Vec<InputNode>,
    pub resource_manager: ResourceManager,
    pub blueprint: Blueprint,

    // State
    pub frame_count: u64,
    pub global_uniforms: GlobalUniforms,
    global_buffer: wgpu::Buffer,
    pub global_bind_group: wgpu::BindGroup,

    // Resource Pools (Persist across frames)
    // We need these to support the RenderGraph pooling
    // (Keys must match what RenderGraph uses internally)
    texture_pool: HashMap<super::render_graph::TextureKey, Vec<Arc<wgpu::Texture>>>,
    buffer_pool: HashMap<u64, Vec<Arc<wgpu::Buffer>>>,

    // Flags & UI State
    pub rebuild_requested: bool,
    pub awaiting_save_map: bool,
    pub selected_node_idx: Option<usize>,

    // Windows
    pub show_control_window: bool,
    pub show_pipeline_window: bool,
    pub show_properties_window: bool,
    pub show_texture_window: bool,
    pub show_shader_window: bool,
    pub editor_text: String,
    pub editor_target_node_idx: Option<usize>,
}

impl Simulation {
    pub fn new(device: &wgpu::Device) -> Self {
        let global_uniforms = GlobalUniforms {
            time: 0.0,
            dt: 0.016,
            frame: 0,
            mouse_btn: 0,
            mouse_pos: [0.0, 0.0],
            screen_size: [100.0, 100.0],
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

        let global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Global BG"),
            layout: &global_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: global_buffer.as_entire_binding(),
            }],
        });

        Self {
            nodes: Vec::new(),
            input_nodes: Vec::new(),
            resource_manager: ResourceManager::new(device),
            blueprint: Blueprint {
                inputs: vec![],
                shared_buffers: vec![],
                nodes: vec![],
            },
            frame_count: 0,
            global_uniforms,
            global_buffer,
            global_bind_group,
            // Pools
            texture_pool: HashMap::new(),
            buffer_pool: HashMap::new(),

            rebuild_requested: false,
            awaiting_save_map: false,
            selected_node_idx: None,
            show_control_window: true,
            show_pipeline_window: true,
            show_properties_window: true,
            show_texture_window: false,
            show_shader_window: false,
            editor_text: String::new(),
            editor_target_node_idx: None,
        }
    }

    pub fn load_blueprint(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut imgui_wgpu::Renderer,
        bp: Blueprint,
        surface_format: wgpu::TextureFormat,
    ) {
        self.blueprint = bp;
        self.nodes.clear();
        self.input_nodes.clear();
        self.resource_manager = ResourceManager::new(device);

        for buf in &self.blueprint.shared_buffers {
            self.resource_manager
                .create_buffer(device, &buf.name, buf.size_bytes);
        }

        for input in &self.blueprint.inputs {
            self.resource_manager.create_texture(
                device,
                renderer,
                &input.name,
                input.width,
                input.height,
                wgpu::TextureFormat::Rgba8Unorm,
            );
            self.input_nodes.push(InputNode::new(
                &input.name,
                input.source.clone(),
                input.width,
                input.height,
            ));
        }

        for node_conf in &self.blueprint.nodes {
            if !node_conf.enabled {
                continue;
            }
            // Create output textures in ResourceManager
            for out_tex in &node_conf.output_textures {
                self.resource_manager.create_texture(
                    device,
                    renderer,
                    out_tex,
                    node_conf.output_width,
                    node_conf.output_height,
                    wgpu::TextureFormat::Rgba32Float,
                );
            }
            let node = BrainNode::new(
                device,
                &self.resource_manager,
                node_conf.clone(),
                surface_format,
            );
            self.nodes.push(node);
        }
        self.rebuild_requested = false;
    }

    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        time: f32,
        dt: f32,
        mouse: (f32, f32),
        mouse_btn: u32,
        size: (f32, f32),
    ) {
        self.frame_count += 1;
        self.global_uniforms.time = time;
        self.global_uniforms.dt = dt;
        self.global_uniforms.frame = self.frame_count as u32;
        self.global_uniforms.mouse_pos = [mouse.0, mouse.1];
        self.global_uniforms.mouse_btn = mouse_btn;
        self.global_uniforms.screen_size = [size.0, size.1];
        queue.write_buffer(
            &self.global_buffer,
            0,
            bytemuck::cast_slice(&[self.global_uniforms]),
        );
        for input in &self.input_nodes {
            input.update(queue, &self.resource_manager);
        }
    }

    /// Executes the frame render using the Directed Acyclic Graph (DAG)
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        camera_bg: &wgpu::BindGroup,
    ) {
        let mut graph = RenderGraph::new();

        // -- Setup Graph Nodes --

        // 1. Compute Pass for Brain Nodes
        // We capture the logic to run the compute shaders.
        // In a fully granular graph, each BrainNode would be a pass.
        // For efficiency here, we group them or add them individually.

        for (i, node) in self.nodes.iter().enumerate() {
            if !node.config.enabled || node.error_msg.is_some() {
                continue;
            }

            // To use the graph correctly, we should declare usage.
            // Since BrainNode internal buffers are persistent, we "Import" them
            // just to notify the graph (optional if we don't use graph dependency sorting yet).
            // But purely for structure, let's define the pass.

            let name = format!("Compute: {}", node.config.id);
            let use_pp = node.config.use_ping_pong;
            let frame = self.frame_count;
            let global_bg = &self.global_bind_group;

            // Note: We can't easily move `node` into the closure because it's borrowed from `self`.
            // The RenderGraph lifetime `'a` allows capturing `&BrainNode`.

            graph.add_pass(
                &name,
                |builder| {
                    // In a full implementation, we would:
                    // builder.read_buffer(graph.import_buffer(node.buffer_a.clone()));
                    // builder.write_buffer(graph.import_buffer(node.buffer_b.clone()));
                    // For now, the builder is just for metadata/sorting hooks in the future.
                    builder
                },
                move |ctx| {
                    let mut cpass = ctx
                        .encoder
                        .begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some(&name),
                            timestamp_writes: None,
                        });

                    cpass.set_pipeline(&node.pipeline);
                    cpass.set_bind_group(1, global_bg, &[]);

                    let bg = if use_pp {
                        if frame % 2 == 0 {
                            &node.bind_group_a
                        } else {
                            node.bind_group_b.as_ref().unwrap()
                        }
                    } else {
                        &node.bind_group_a
                    };

                    cpass.set_bind_group(0, bg, &[]);
                    let groups = (node.config.count + 63) / 64;
                    cpass.dispatch_workgroups(groups, 1, 1);
                },
            );
        }

        // 2. Async Tasks Pass (Save)
        if self.awaiting_save_map {
            graph.add_pass(
                "Save Request",
                |_| {},
                move |ctx| {
                    for node in &self.nodes {
                        node.request_save(ctx.encoder);
                    }
                },
            );
        }

        // 3. Visualization Pass
        // We can create a transient intermediate texture here to demonstrate pooling features,
        // but ultimately we need to write to the Swapchain `view`.
        // Since `view` is a TextureView (not Texture), we handle it as an External Reference
        // passed directly to the closure, or wrapping it.

        // Let's demonstrate Graph Memory: Create a dummy bloom-like buffer (just to show allocation)
        // and then render to Main.
        let screen_size = wgpu::Extent3d {
            width: self.global_uniforms.screen_size[0] as u32,
            height: self.global_uniforms.screen_size[1] as u32,
            depth_or_array_layers: 1,
        };

        // Example of creating a transient texture via Graph
        // (We won't actually draw to it to save performance in this demo, but this proves the API)
        let _intermediate_handle = graph.create_texture(wgpu::TextureDescriptor {
            label: Some("Intermediate Graph Tex"),
            size: screen_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        graph.add_pass(
            "Visualization",
            |builder| builder, // Declare dependencies here if we used the intermediate
            move |ctx| {
                let mut rpass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Vis Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view, // Writing directly to swapchain
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

                for node in &self.nodes {
                    if node.config.enabled && node.config.visualize {
                        if let Some(vis) = &node.visualizer {
                            rpass.set_pipeline(&vis.pipeline);
                            rpass.set_bind_group(1, camera_bg, &[]);

                            let bg = if node.config.use_ping_pong {
                                if self.frame_count % 2 == 0 {
                                    node.viz_bg_a.as_ref().unwrap()
                                } else {
                                    node.viz_bg_b.as_ref().unwrap()
                                }
                            } else {
                                node.viz_bg_a.as_ref().unwrap()
                            };

                            rpass.set_bind_group(0, bg, &[]);
                            match node.config.primitive_topology.as_str() {
                                "LineList" => rpass.draw(0..(node.config.count * 2), 0..1),
                                _ => rpass.draw(0..6, 0..node.config.count),
                            }
                        }
                    }
                }
            },
        );

        // -- Execute Graph --
        graph.execute(
            device,
            queue,
            encoder,
            &mut self.texture_pool,
            &mut self.buffer_pool,
        );
    }

    pub fn post_submit(&mut self, device: &wgpu::Device) {
        if self.awaiting_save_map {
            for node in &self.nodes {
                let slice = node.staging_buffer.slice(..);
                let (tx, rx) = std::sync::mpsc::channel();
                slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                device.poll(wgpu::Maintain::Wait);
                if let Ok(Ok(())) = rx.recv() {
                    let data = slice.get_mapped_range();
                    let _ = fs::create_dir_all("saves");
                    let _ = fs::write(format!("saves/{}.bin", node.config.id), &*data);
                    drop(data);
                    node.staging_buffer.unmap();
                }
            }
            self.awaiting_save_map = false;
        }
    }

    pub fn save_all_synchronous(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut encoder = device.create_command_encoder(&Default::default());
        for node in &self.nodes {
            node.request_save(&mut encoder);
        }
        queue.submit(Some(encoder.finish()));

        for node in &self.nodes {
            let slice = node.staging_buffer.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::Maintain::Wait);
            if let Ok(Ok(())) = rx.recv() {
                let data = slice.get_mapped_range();
                let _ = fs::create_dir_all("saves");
                let _ = fs::write(format!("saves/{}.bin", node.config.id), &*data);
                drop(data);
                node.staging_buffer.unmap();
            }
        }
    }

    pub fn render_ui(&mut self, ui: &Ui, queue: &wgpu::Queue) {
        if let Some(menu) = ui.begin_main_menu_bar() {
            if let Some(win_menu) = ui.begin_menu("Windows") {
                ui.checkbox("Control Panel", &mut self.show_control_window);
                ui.checkbox("Pipeline Editor", &mut self.show_pipeline_window);
                ui.checkbox("Properties", &mut self.show_properties_window);
                ui.checkbox("Texture Browser", &mut self.show_texture_window);
                ui.checkbox("Shader Editor", &mut self.show_shader_window);
                win_menu.end();
            }
            menu.end();
        }

        if self.show_control_window {
            self.window_control(ui);
        }
        if self.show_pipeline_window {
            self.window_pipeline_editor(ui);
        }
        if self.show_properties_window {
            self.window_properties(ui, queue);
        }
        if self.show_texture_window {
            self.window_texture_browser(ui);
        }
        if self.show_shader_window {
            self.window_shader_editor(ui);
        }
    }

    fn window_control(&mut self, ui: &Ui) {
        ui.window("Control Panel")
            .opened(&mut self.show_control_window)
            .build(|| {
                ui.text(format!("FPS: {:.1}", ui.io().framerate));
                ui.text(format!("Frame: {}", self.frame_count));
                ui.separator();
                if ui.button("Save Weights") {
                    self.awaiting_save_map = true;
                }
                ui.same_line();
                if ui.button("Reload Graph") {
                    self.rebuild_requested = true;
                }
            });
    }

    fn window_pipeline_editor(&mut self, ui: &Ui) {
        ui.window("Pipeline Editor")
            .opened(&mut self.show_pipeline_window)
            .build(|| {
                ui.text("Render Graph Architecture Active");
                ui.separator();

                if CollapsingHeader::new("Shared Buffers").build(ui) {
                    let mut remove_buf = None;
                    for (i, buf) in self.blueprint.shared_buffers.iter_mut().enumerate() {
                        let _id = ui.push_id(format!("sbuf_{}", i));
                        ui.input_text("Name", &mut buf.name).build();
                        let mut mb = (buf.size_bytes / 1024 / 1024) as i32;
                        if ui.input_int("Size (MB)", &mut mb).build() {
                            buf.size_bytes = (mb.max(1) as u64) * 1024 * 1024;
                        }
                        if ui.button("Del") {
                            remove_buf = Some(i);
                        }
                    }
                    if let Some(i) = remove_buf {
                        self.blueprint.shared_buffers.remove(i);
                    }
                    if ui.button("+ Add Buffer") {
                        self.blueprint
                            .shared_buffers
                            .push(super::blueprint::SharedBufferConfig {
                                name: "NewBuffer".into(),
                                size_bytes: 1024 * 1024,
                                label: None,
                            });
                    }
                }

                ui.separator();
                ui.text("Nodes:");

                // Simple Node list for UI
                for (i, node) in self.blueprint.nodes.iter_mut().enumerate() {
                    let _id = ui.push_id(i.to_string());
                    if ui
                        .selectable_config(&node.id)
                        .selected(self.selected_node_idx == Some(i))
                        .build()
                    {
                        self.selected_node_idx = Some(i);
                    }
                }
            });
    }

    fn window_properties(&mut self, ui: &Ui, queue: &wgpu::Queue) {
        ui.window("Properties")
            .opened(&mut self.show_properties_window)
            .build(|| {
                if let Some(idx) = self.selected_node_idx {
                    if let Some(node) = self.nodes.get_mut(idx) {
                        ui.text_colored([0.0, 1.0, 0.0, 1.0], format!("Node: {}", node.config.id));
                        ui.separator();
                        for (i, param) in node.params.iter_mut().enumerate() {
                            if ui.slider(format!("Param {}", i), 0.0, 1.0, param) {
                                node.update_params(queue);
                            }
                        }
                    }
                } else {
                    ui.text("Select a node to edit properties.");
                }
            });
    }

    fn window_texture_browser(&self, ui: &Ui) {
        ui.window("Texture Browser")
            .opened(&mut self.show_texture_window.clone())
            .build(|| {
                for (name, _) in &self.resource_manager.textures {
                    ui.text(name);
                    // Image preview logic would go here using Imgui IDs from resource manager
                }
            });
    }

    fn window_shader_editor(&mut self, ui: &Ui) {
        ui.window("Shader Editor")
            .opened(&mut self.show_shader_window)
            .build(|| {
                ui.text("Select node to edit shader source...");
                // Text editor logic would go here
            });
    }
}
