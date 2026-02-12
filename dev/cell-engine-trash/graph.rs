use super::blueprint::{
    Blueprint, BufferAccess, InputConfig, InputSource, NodeConfig, SharedBufferConfig,
};
use super::inputs::InputNode;
use super::node::BrainNode;
use super::resources::ResourceManager;
use imgui::*;
use std::fs;
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

pub struct RenderGraph {
    pub nodes: Vec<BrainNode>,
    pub input_nodes: Vec<InputNode>,
    pub resource_manager: ResourceManager,
    pub blueprint: Blueprint,
    pub rebuild_requested: bool,
    pub frame_count: u64,
    global_uniforms: GlobalUniforms,
    global_buffer: wgpu::Buffer,
    pub global_bind_group: wgpu::BindGroup,
    pub selected_node_idx: Option<usize>,
    pub show_control_window: bool,
    pub show_pipeline_window: bool,
    pub show_shader_window: bool,
    pub show_texture_window: bool,
    pub show_properties_window: bool,
    editor_text: String,
    editor_target_node_idx: Option<usize>,
    awaiting_save_map: bool,
}

impl RenderGraph {
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
            rebuild_requested: false,
            frame_count: 0,
            global_uniforms,
            global_buffer,
            global_bind_group,
            selected_node_idx: None,
            show_control_window: true,
            show_pipeline_window: true,
            show_shader_window: true,
            show_texture_window: false,
            show_properties_window: true,
            editor_text: String::new(),
            editor_target_node_idx: None,
            awaiting_save_map: false,
        }
    }

    pub fn load_blueprint(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut imgui_wgpu::Renderer,
        bp: Blueprint,
        surface_format: wgpu::TextureFormat,
    ) {
        self.blueprint = bp.clone();
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

    pub fn compute(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Graph Compute"),
            timestamp_writes: None,
        });
        cpass.set_bind_group(1, &self.global_bind_group, &[]);

        for node in &self.nodes {
            if !node.config.enabled {
                continue;
            }
            if node.error_msg.is_some() {
                continue;
            }

            cpass.set_pipeline(&node.pipeline);

            // FIXED: Select the WRITE buffer based on frame parity
            let bg = if node.config.use_ping_pong {
                if self.frame_count % 2 == 0 {
                    &node.bind_group_a // Even frames: Write to A, Read from B
                } else {
                    node.bind_group_b.as_ref().unwrap() // Odd frames: Write to B, Read from A
                }
            } else {
                &node.bind_group_a
            };

            cpass.set_bind_group(0, bg, &[]);
            let groups = (node.config.count + 63) / 64;
            cpass.dispatch_workgroups(groups, 1, 1);
        }
    }

    pub fn visualize(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        camera_bg: &wgpu::BindGroup,
        depth: &wgpu::TextureView,
    ) {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Vis Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
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

                    // FIXED: Read from the buffer that was WRITTEN TO in the last compute pass
                    let bg = if node.config.use_ping_pong {
                        if self.frame_count % 2 == 0 {
                            // Even frame: We just wrote to A, so visualize A
                            node.viz_bg_a.as_ref().unwrap()
                        } else {
                            // Odd frame: We just wrote to B, so visualize B
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

                ui.separator();
                ui.text("Textures:");

                // Iterate over all resources, filtering for textures
                let avail_w = ui.content_region_avail()[0];
                let size = [avail_w, avail_w * 0.75]; // Aspect ratio

                for (name, _) in &self.resource_manager.textures {
                    ui.group(|| {
                        Self::draw_texture_preview(ui, &self.resource_manager, name, name, size);
                    });
                    ui.separator();
                }
            });
    }

    fn window_pipeline_editor(&mut self, ui: &Ui) {
        ui.window("Pipeline Editor")
            .opened(&mut self.show_pipeline_window)
            .build(|| {
                if ui.button("Save Config") {
                    if let Ok(json) = serde_json::to_string_pretty(&self.blueprint) {
                        let _ = fs::write("assets/brain_config.json", json);
                    }
                }
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
                        self.blueprint.shared_buffers.push(SharedBufferConfig {
                            name: "NewBuffer".into(),
                            size_bytes: 1024 * 1024,
                            label: None,
                        });
                    }
                }

                ui.separator();
                ui.text("Process Chain:");

                let mut node_to_remove = None;
                for (i, node) in self.blueprint.nodes.iter_mut().enumerate() {
                    let _id = ui.push_id(i.to_string());
                    let is_selected = self.selected_node_idx == Some(i);

                    if ui
                        .selectable_config(format!("{}##sel", node.id))
                        .selected(is_selected)
                        .build()
                    {
                        self.selected_node_idx = Some(i);
                        self.editor_target_node_idx = Some(i);
                        if let Ok(code) = fs::read_to_string(&node.shader_path) {
                            self.editor_text = code;
                        }
                    }

                    if is_selected {
                        ui.indent();
                        ui.text_colored([0.4, 1.0, 0.4, 1.0], "Node Settings");
                        ui.input_text("ID", &mut node.id).build();
                        ui.checkbox("Enabled", &mut node.enabled);
                        ui.input_text("Shader", &mut node.shader_path).build();
                        ui.input_text("Entry Point", &mut node.entry_point).build();

                        let mut count = node.count as i32;
                        if ui.input_int("Count", &mut count).build() {
                            node.count = count.max(1) as u32;
                        }

                        if CollapsingHeader::new("Memory Layout").build(ui) {
                            ui.input_text("Struct Name", &mut node.struct_name).build();
                            let mut sz = node.struct_size_bytes as i32;
                            if ui.input_int("Struct Size (B)", &mut sz).build() {
                                node.struct_size_bytes = sz.max(4) as u64;
                            }

                            if ui.checkbox("External Memory", &mut node.external_memory.is_some()) {
                                if node.external_memory.is_some() {
                                    node.external_memory = None;
                                } else {
                                    node.external_memory = Some("Neurons".to_string());
                                }
                            }
                            if let Some(ext) = &mut node.external_memory {
                                ui.input_text("Shared Buffer Name", ext).build();
                            }
                            ui.checkbox("Ping Pong", &mut node.use_ping_pong);
                        }

                        if CollapsingHeader::new("Bindings").build(ui) {
                            ui.text("Access Buffers:");
                            let mut rem_acc = None;
                            for (bi, acc) in node.access_buffers.iter_mut().enumerate() {
                                let _bid = ui.push_id(bi.to_string());
                                ui.set_next_item_width(100.0);
                                ui.input_text("", &mut acc.name).build();
                                ui.same_line();
                                ui.checkbox("W", &mut acc.writable);
                                ui.same_line();
                                if ui.button("x") {
                                    rem_acc = Some(bi);
                                }
                            }
                            if let Some(r) = rem_acc {
                                node.access_buffers.remove(r);
                            }
                            if ui.button("+ Access") {
                                node.access_buffers.push(BufferAccess {
                                    name: "SpatialGrid".into(),
                                    type_name: "atomic<u32>".into(),
                                    writable: false,
                                    shader_name: None,
                                });
                            }

                            ui.text("Textures:");

                            TreeNode::new("Inputs", ui).build(|| {
                                let mut rem = None;
                                for (ti, t) in node.input_textures.iter_mut().enumerate() {
                                    let _tid = ui.push_id(format!("in_{}", ti));
                                    ui.input_text("", t).build();
                                    ui.same_line();
                                    if ui.button("x") {
                                        rem = Some(ti);
                                    }
                                }
                                if let Some(r) = rem {
                                    node.input_textures.remove(r);
                                }
                                if ui.button("+") {
                                    node.input_textures.push("Camera".into());
                                }
                            });

                            TreeNode::new("Outputs", ui).build(|| {
                                let mut rem = None;
                                for (ti, t) in node.output_textures.iter_mut().enumerate() {
                                    let _tid = ui.push_id(format!("out_{}", ti));
                                    ui.input_text("", t).build();
                                    ui.same_line();
                                    if ui.button("x") {
                                        rem = Some(ti);
                                    }
                                }
                                if let Some(r) = rem {
                                    node.output_textures.remove(r);
                                }
                                if ui.button("+") {
                                    node.output_textures.push("NewTex".into());
                                }
                            });
                        }

                        if CollapsingHeader::new("Visualization").build(ui) {
                            ui.checkbox("Active", &mut node.visualize);
                            if node.visualize {
                                if let Some(vpath) = &mut node.visualizer_shader_path {
                                    ui.input_text("Vis Shader", vpath).build();
                                } else if ui.button("Set Custom Shader") {
                                    node.visualizer_shader_path =
                                        Some("src/shaders/vis/default.wgsl".into());
                                }

                                let items = ["PointList", "LineList", "TriangleList"];
                                let mut current_item = items
                                    .iter()
                                    .position(|&x| x == node.primitive_topology)
                                    .unwrap_or(2);
                                if ui.combo_simple_string("Topology", &mut current_item, &items) {
                                    node.primitive_topology = items[current_item].to_string();
                                }
                            }
                        }

                        if ui.button("Delete Node") {
                            node_to_remove = Some(i);
                        }
                        ui.unindent();
                    }
                }

                if let Some(i) = node_to_remove {
                    self.blueprint.nodes.remove(i);
                    self.selected_node_idx = None;
                }

                ui.separator();
                if ui.button("+ Add Node") {
                    self.blueprint.nodes.push(NodeConfig::default());
                }
            });
    }

    fn window_properties(&mut self, ui: &Ui, queue: &wgpu::Queue) {
        ui.window("Properties")
            .opened(&mut self.show_properties_window)
            .build(|| {
                if let Some(idx) = self.selected_node_idx {
                    if idx < self.nodes.len() {
                        let node = &mut self.nodes[idx];
                        ui.text_colored([0.5, 1.0, 0.5, 1.0], format!("LIVE: {}", node.config.id));
                        if let Some(err) = &node.error_msg {
                            ui.text_colored([1.0, 0.0, 0.0, 1.0], format!("Error: {}", err));
                        }
                        ui.separator();
                        if CollapsingHeader::new("Parameters")
                            .default_open(true)
                            .build(ui)
                        {
                            let mut changed = false;
                            for (i, param) in node.params.iter_mut().enumerate() {
                                if ui.slider(format!("p{}", i), 0.0, 1.0, param) {
                                    changed = true;
                                }
                            }
                            if changed {
                                node.update_params(queue);
                            }
                        }
                    } else {
                        ui.text("Node not rebuilt yet.");
                    }
                } else {
                    ui.text("Select a node.");
                }
            });
    }

    fn window_shader_editor(&mut self, ui: &Ui) {
        ui.window("Shader Editor")
            .opened(&mut self.show_shader_window)
            .build(|| {
                if let Some(idx) = self.editor_target_node_idx {
                    if let Some(node_conf) = self.blueprint.nodes.get(idx) {
                        ui.text(format!("Editing: {}", node_conf.shader_path));
                        if ui.button("Save") {
                            let _ = fs::write(&node_conf.shader_path, &self.editor_text);
                        }
                        let avail = ui.content_region_avail();
                        ui.input_text_multiline(
                            "##src",
                            &mut self.editor_text,
                            [avail[0], avail[1] - 30.0],
                        )
                        .build();
                    }
                }
            });
    }

    fn window_texture_browser(&self, ui: &Ui) {
        ui.window("Texture Browser")
            .opened(&mut self.show_texture_window.clone())
            .build(|| {
                let avail_w = ui.content_region_avail()[0];
                let cols = (avail_w / 160.0).floor() as i32;
                ui.columns(cols.max(1), "tex_grid", false);
                for (name, _) in &self.resource_manager.textures {
                    ui.group(|| {
                        Self::draw_texture_preview(
                            ui,
                            &self.resource_manager,
                            name,
                            name,
                            [150.0, 150.0],
                        );
                    });
                    ui.next_column();
                }
                ui.columns(1, "reset", false);
            });
    }

    fn draw_texture_preview(
        ui: &Ui,
        mgr: &ResourceManager,
        name: &str,
        label: &str,
        size: [f32; 2],
    ) {
        if let Some(res) = mgr.textures.get(name) {
            if let Some(id) = res.imgui_id {
                Image::new(id, size).build(ui);
                if ui.is_item_hovered() {
                    ui.tooltip_text(format!("{} ({}x{})", name, res.size.width, res.size.height));
                }
            } else {
                ui.text("No ID");
            }
        } else {
            ui.text("Missing");
        }
        ui.text(label);
    }

    pub fn handle_async_tasks(
        &mut self,
        _device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if self.awaiting_save_map {
            for node in &self.nodes {
                node.request_save(encoder);
            }
        }
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

    /// Debug helper: Print current buffer states
    pub fn debug_print_buffer_state(&self) {
        println!("\n=== FRAME {} DEBUG ===", self.frame_count);
        println!(
            "Frame parity: {}",
            if self.frame_count % 2 == 0 {
                "EVEN"
            } else {
                "ODD"
            }
        );

        for (i, node) in self.nodes.iter().enumerate() {
            if !node.config.use_ping_pong {
                continue;
            }

            println!("\nNode {}: {}", i, node.config.id);
            println!("  Ping-pong: ENABLED");
            println!("  Buffer A exists: {}", node.buffer_a.size() > 0);
            println!("  Buffer B exists: {}", node.buffer_b.is_some());

            if self.frame_count % 2 == 0 {
                println!("  Current operation: READ from A, WRITE to B");
                println!("  Using: bind_group_a");
                println!("  Visualizing: buffer B (viz_bg_b)");
            } else {
                println!("  Current operation: READ from B, WRITE to A");
                println!("  Using: bind_group_b");
                println!("  Visualizing: buffer A (viz_bg_a)");
            }
        }
        println!("================\n");
    }

    /// Debug helper: Check if neurons are initialized
    pub async fn check_neuron_initialization(&self, device: &wgpu::Device) {
        for (i, node) in self.nodes.iter().enumerate() {
            println!("\n=== Checking Node {}: {} ===", i, node.config.id);

            // Create staging buffer
            let size = node.config.struct_size_bytes * node.config.count as u64;
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Debug Staging"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            // Copy buffer A
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(&node.buffer_a, 0, &staging, 0, size.min(4800)); // First 100 neurons
            self.queue.submit(Some(encoder.finish()));

            // Read back
            let slice = staging.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::Maintain::Wait);

            if let Ok(Ok(())) = rx.recv() {
                let data = slice.get_mapped_range();

                // Interpret as Neuron structs (48 bytes each)
                let mut initialized_count = 0;
                let mut active_count = 0;

                for neuron_idx in 0..100.min(node.config.count) {
                    let offset = (neuron_idx as usize) * 48;
                    if offset + 48 > data.len() {
                        break;
                    }

                    // Read fatigue field (offset 40, f32)
                    let fatigue_bytes = &data[offset + 40..offset + 44];
                    let fatigue = f32::from_le_bytes([
                        fatigue_bytes[0],
                        fatigue_bytes[1],
                        fatigue_bytes[2],
                        fatigue_bytes[3],
                    ]);

                    // Read voltage field (offset 16, f32)
                    let voltage_bytes = &data[offset + 16..offset + 20];
                    let voltage = f32::from_le_bytes([
                        voltage_bytes[0],
                        voltage_bytes[1],
                        voltage_bytes[2],
                        voltage_bytes[3],
                    ]);

                    if fatigue > 0.0 {
                        initialized_count += 1;
                        if voltage > 0.1 {
                            active_count += 1;
                        }
                    }
                }

                drop(data);
                staging.unmap();

                println!("  Sampled {} neurons:", 100.min(node.config.count));
                println!("    Initialized: {}", initialized_count);
                println!("    Active (voltage > 0.1): {}", active_count);

                if initialized_count == 0 {
                    println!("  ⚠️  WARNING: No neurons initialized! Check initialization logic.");
                } else if active_count == 0 {
                    println!(
                        "  ⚠️  WARNING: Neurons initialized but none active! Check update logic."
                    );
                } else {
                    println!("  ✓ Network appears to be running.");
                }
            }
        }
    }
}
