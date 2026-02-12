use super::blueprint::{Blueprint, PassConfig, ResourceConfig};
use super::engine::Engine;
use imgui::*;
use imgui_wgpu::Renderer;
use std::fs;
use std::path::Path;
use wgpu::Queue;

pub struct Simulation {
    pub engine: Engine,
    // UI State
    selected_buffer: String,
    view_as_float: bool,

    // Blueprint Selection State
    pub available_blueprints: Vec<String>,
    pub current_blueprint_idx: usize,
    pub pending_blueprint_path: Option<String>,
}

impl Simulation {
    pub fn new(device: &wgpu::Device) -> Self {
        // 1. Scan assets folder for json files
        let mut available_blueprints = Vec::new();
        if let Ok(entries) = fs::read_dir("assets") {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "json" {
                        let path_str = path.to_string_lossy().replace("\\", "/");
                        available_blueprints.push(path_str);
                    }
                }
            }
        }
        available_blueprints.sort();

        let default_bp = "assets/neuron_driver_v3.json";
        let current_blueprint_idx = available_blueprints
            .iter()
            .position(|p| p == default_bp)
            .unwrap_or(0);

        let initial_path = if !available_blueprints.is_empty() {
            &available_blueprints[current_blueprint_idx]
        } else {
            default_bp
        };

        Self {
            engine: Engine::new(device, initial_path),
            selected_buffer: String::new(),
            view_as_float: true,
            available_blueprints,
            current_blueprint_idx,
            pending_blueprint_path: None,
        }
    }

    pub fn load_blueprint(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut Renderer,
        blueprint: Blueprint,
        surface_format: wgpu::TextureFormat,
    ) {
        self.engine
            .load_graph_from_blueprint(device, blueprint, surface_format);
        self.engine.register_imgui_textures(renderer, device);
    }

    pub fn update(
        &mut self,
        queue: &Queue,
        time: f32,
        dt: f32,
        mouse_ndc: (f32, f32),
        _mouse_buttons: u32,
        screen_size: (f32, f32),
    ) {
        let mouse = [mouse_ndc.0, mouse_ndc.1, 0.0, 0.0];
        let screen = [screen_size.0, screen_size.1];
        self.engine.update(queue, time, dt, mouse, screen);
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        backbuffer_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        _camera_bind_group: &wgpu::BindGroup,
    ) {
        self.engine
            .render(device, queue, encoder, backbuffer_view, depth_view);
    }

    pub fn render_ui(&mut self, ui: &imgui::Ui, device: &wgpu::Device, queue: &wgpu::Queue) {
        ui.window("Simulation Control")
            .size([400.0, 600.0], imgui::Condition::FirstUseEver)
            .build(|| {
                if ui.collapsing_header("Stats", TreeNodeFlags::DEFAULT_OPEN) {
                    ui.text(format!("Frame: {}", self.engine.frame_count));
                    ui.text("Press TAB to toggle Camera/Cursor");
                }

                // --- BLUEPRINT SELECTOR ---
                ui.separator();
                ui.text("Blueprint Selection");

                let mut selected_changed = false;
                let current_name = if self.available_blueprints.is_empty() {
                    "No JSONs found".to_string()
                } else {
                    Path::new(&self.available_blueprints[self.current_blueprint_idx])
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                };

                if let Some(token) = ui.begin_combo("Scene", &current_name) {
                    for (i, path) in self.available_blueprints.iter().enumerate() {
                        let display_name = Path::new(path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy();

                        let is_selected = i == self.current_blueprint_idx;
                        if ui
                            .selectable_config(&display_name)
                            .selected(is_selected)
                            .build()
                        {
                            self.current_blueprint_idx = i;
                            selected_changed = true;
                        }
                        if is_selected {
                            ui.set_item_default_focus();
                        }
                    }
                    token.end();
                }

                if selected_changed {
                    let path = self.available_blueprints[self.current_blueprint_idx].clone();
                    self.pending_blueprint_path = Some(path);
                }
                ui.separator();

                // --- Pipeline Graph Visualization ---
                if ui.collapsing_header("Pipeline Graph", TreeNodeFlags::DEFAULT_OPEN) {
                    for (i, pass) in self.engine.config.passes.iter().enumerate() {
                        let pass_name = match pass {
                            PassConfig::Compute { name, .. } => format!("[Compute] {}", name),
                            PassConfig::Render { name, .. } => format!("[Render] {}", name),
                            PassConfig::Copy { name, .. } => format!("[Copy] {}", name),
                        };

                        if ui.tree_node(format!("Pass {}: {}", i, pass_name)).is_some() {
                            match pass {
                                PassConfig::Compute { inputs, .. }
                                | PassConfig::Render { inputs, .. } => {
                                    if inputs.is_empty() {
                                        ui.text_disabled("No inputs");
                                    }
                                    for input in inputs {
                                        ui.text(format!(
                                            "  Binding {}: {}",
                                            input.binding, input.resource
                                        ));
                                        if input.writable {
                                            ui.same_line();
                                            ui.text_colored([1.0, 0.5, 0.5, 1.0], "(Write)");
                                        }
                                    }
                                }
                                PassConfig::Copy {
                                    source,
                                    destination,
                                    ..
                                } => {
                                    ui.text(format!("  Src: {}", source));
                                    ui.text(format!("  Dst: {}", destination));
                                }
                            }
                        }
                    }
                }

                // --- Texture Inspector ---
                if ui.collapsing_header("Textures", TreeNodeFlags::DEFAULT_OPEN) {
                    // Create a sorted list of names for stable UI ordering
                    let mut texture_names: Vec<_> = self.engine.imgui_texture_map.keys().collect();
                    texture_names.sort();

                    for name in texture_names {
                        if let Some(id) = self.engine.imgui_texture_map.get(name) {
                            if ui.tree_node(name).is_some() {
                                // Default dimensions
                                let mut display_size = [256.0, 256.0];
                                let mut native_size_str = String::from("Unknown");

                                // Look up actual texture size in ResourceManager
                                if let Some(crate::framework::resources::GpuResource::Texture(
                                    bundle,
                                )) = self.engine.resource_manager.resources.get(name)
                                {
                                    let w = bundle.size.width as f32;
                                    let h = bundle.size.height as f32;
                                    native_size_str = format!("{}x{}", w, h);

                                    // Calculate aspect-preserved size constrained to max 256
                                    let max_dim = 256.0;
                                    let scale = if w > max_dim || h > max_dim {
                                        (max_dim / w).min(max_dim / h)
                                    } else {
                                        1.0
                                    };
                                    display_size = [w * scale, h * scale];
                                }

                                ui.text(format!("Native Size: {}", native_size_str));
                                Image::new(*id, display_size).build(ui);
                            }
                        }
                    }
                }

                // --- Buffer Inspector ---
                if ui.collapsing_header("Buffer Inspector", TreeNodeFlags::DEFAULT_OPEN) {
                    if let Some(token) = ui.begin_combo("Target Buffer", &self.selected_buffer) {
                        for res in &self.engine.config.resources {
                            if let ResourceConfig::Buffer { name, .. } = res {
                                let is_selected = self.selected_buffer == *name;
                                if ui.selectable_config(name).selected(is_selected).build() {
                                    self.selected_buffer = name.clone();
                                }
                                if is_selected {
                                    ui.set_item_default_focus();
                                }
                            }
                        }
                        token.end();
                    }

                    if ui.button("Capture from GPU") {
                        if !self.selected_buffer.is_empty() {
                            self.engine
                                .capture_buffer(device, queue, &self.selected_buffer);
                        }
                    }
                    ui.same_line();
                    ui.checkbox("View as Floats", &mut self.view_as_float);

                    if let Some((name, data)) = &self.engine.inspector_buffer_data {
                        if name == &self.selected_buffer {
                            ui.separator();
                            ui.text(format!("Data Size: {} bytes", data.len()));

                            ui.child_window("buffer_data")
                                .size([0.0, 300.0])
                                .border(true)
                                .build(|| {
                                    let total_items = data.len() / 4;
                                    let clipper =
                                        imgui::ListClipper::new(total_items as i32).begin(ui);
                                    for row in clipper.iter() {
                                        let i: usize = row as usize * 4 as usize;
                                        if i + 4 <= data.len() {
                                            let bytes = &data[i..i + 4];
                                            let label = format!("{:04X}", i);

                                            if self.view_as_float {
                                                let val =
                                                    f32::from_le_bytes(bytes.try_into().unwrap());
                                                ui.text(format!("{}: {:.4}", label, val));
                                            } else {
                                                ui.text(format!(
                                                    "{}: {:02X} {:02X} {:02X} {:02X}",
                                                    label, bytes[0], bytes[1], bytes[2], bytes[3]
                                                ));
                                            }
                                        }
                                    }
                                });
                        }
                    }
                }
            });
    }

    pub fn post_submit(&mut self, _device: &wgpu::Device) {}

    pub fn save_all_synchronous(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        log::info!("Saving simulation state (Not implemented yet)");
    }
}
