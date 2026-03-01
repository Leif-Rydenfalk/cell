//! UI Cell - Dear ImGui + Layout Engine
//!
//! This cell provides:
//! - Dear ImGui integration
//! - Layout management
//! - API for other cells to create menus
//! - Theme system

use anyhow::Result;
use cell_sdk::*;
use imgui::*;
use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::RwLock;
use tokio::sync::Mutex;

// ========= PROTEINS (PUBLIC API) =========

#[protein]
pub struct RegisterWindow {
    pub window_id: String,
    pub title: String,
    pub initial_position: Option<[f32; 2]>,
    pub initial_size: Option<[f32; 2]>,
    pub flags: Vec<WindowFlag>,
}

#[protein]
pub enum WindowFlag {
    NoTitleBar,
    NoResize,
    NoMove,
    NoScrollbar,
    NoCollapse,
    AlwaysAutoResize,
    NoBackground,
    NoSavedSettings,
    MenuBar,
    HorizontalScrollbar,
    NoFocusOnAppearing,
    NoBringToFrontOnFocus,
    AlwaysVerticalScrollbar,
    AlwaysHorizontalScrollbar,
    NoNavInputs,
    NoNavFocus,
    UnsavedDocument,
    NoDocking,
}

#[protein]
pub struct UnregisterWindow {
    pub window_id: String,
}

#[protein]
pub struct ShowWindow {
    pub window_id: String,
    pub visible: bool,
}

#[protein]
pub struct AddMenuItem {
    pub menu_path: String,
    pub label: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub window_id: Option<String>,
}

#[protein]
pub struct AddWidget {
    pub window_id: String,
    pub widget: Widget,
}

#[protein]
pub enum Widget {
    Button {
        id: String,
        label: String,
        size: Option<[f32; 2]>,
    },
    Checkbox {
        id: String,
        label: String,
        initial: bool,
    },
    SliderFloat {
        id: String,
        label: String,
        min: f32,
        max: f32,
        initial: f32,
        format: String,
    },
    SliderInt {
        id: String,
        label: String,
        min: i32,
        max: i32,
        initial: i32,
        format: String,
    },
    InputText {
        id: String,
        label: String,
        initial: String,
        buffer_size: usize,
        multiline: bool,
    },
    ColorPicker {
        id: String,
        label: String,
        initial: [f32; 4],
    },
    ComboBox {
        id: String,
        label: String,
        items: Vec<String>,
        initial: usize,
    },
    ListBox {
        id: String,
        label: String,
        items: Vec<String>,
        initial: Vec<usize>,
        multi_select: bool,
    },
    Plot {
        id: String,
        label: String,
        values: Vec<f32>,
        overlay_text: Option<String>,
        scale_min: Option<f32>,
        scale_max: Option<f32>,
        graph_size: [f32; 2],
    },
    ProgressBar {
        id: String,
        fraction: f32,
        overlay: Option<String>,
    },
    Separator,
    Text {
        id: String,
        content: String,
        color: Option<[f32; 4]>,
        wrap_width: Option<f32>,
    },
    Image {
        id: String,
        texture_id: String,
        size: [f32; 2],
        uv_min: [f32; 2],
        uv_max: [f32; 2],
        tint_color: Option<[f32; 4]>,
        border_color: Option<[f32; 4]>,
    },
}

#[protein]
pub struct WidgetUpdate {
    pub window_id: String,
    pub widget_id: String,
    pub value: WidgetValue,
}

#[protein]
pub enum WidgetValue {
    Bool(bool),
    Float(f32),
    Int(i32),
    String(String),
    Color([f32; 4]),
    Selection(usize),
    MultiSelection(Vec<usize>),
}

#[protein]
pub struct WidgetEvent {
    pub window_id: String,
    pub widget_id: String,
    pub event_type: WidgetEventType,
    pub value: Option<WidgetValue>,
}

#[protein]
pub enum WidgetEventType {
    Clicked,
    Changed,
    Activated,
    Deactivated,
    Hovered,
    Focused,
}

#[protein]
pub struct SetTheme {
    pub theme: Theme,
}

#[protein]
pub struct Theme {
    pub colors: std::collections::HashMap<String, [f32; 4]>,
    pub style_vars: std::collections::HashMap<String, f32>,
    pub font_size: f32,
    pub rounding: f32,
}

#[protein]
pub struct BeginFrame;
#[protein]
pub struct EndFrame;

#[protein]
pub struct RenderUI {
    pub render_target: RenderTarget,
}

#[protein]
pub enum RenderTarget {
    Texture { width: u32, height: u32, format: String },
    Screen,
}

#[protein]
pub struct UIFrame {
    pub draw_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

// ========= INTERNAL STATE =========

// Wrapper to make ImGui context Send+Sync for our actor model
// SAFETY: We ensure exclusive access via RwLock in the Service
pub struct ImGuiWrapper(pub imgui::Context);
unsafe impl Send for ImGuiWrapper {}
unsafe impl Sync for ImGuiWrapper {}

struct WindowState {
    id: String,
    title: String,
    visible: bool,
    position: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    flags: Vec<WindowFlag>,
    widgets: HashMap<String, Widget>,
    widget_states: HashMap<String, WidgetState>,
    // Removed menu_items from window state to simplify
}

struct WidgetState {
    value: WidgetValue,
    last_frame_active: u64,
}

struct MenuItem {
    path: String,
    label: String,
    shortcut: Option<String>,
    enabled: bool,
    window_id: Option<String>,
}

struct UIState {
    imgui: ImGuiWrapper,
    windows: HashMap<String, WindowState>,
    menu_items: Vec<MenuItem>,
    global_menu_bar: bool,
    current_frame: u64,
    theme: Theme,
}

impl UIState {
    fn new() -> Self {
        let mut imgui = imgui::Context::create();
        imgui.set_ini_filename(None);
        
        // Default style setup
        let style = imgui.style_mut();
        style.window_rounding = 4.0;
        style.frame_rounding = 3.0;
        
        Self {
            imgui: ImGuiWrapper(imgui),
            windows: HashMap::new(),
            menu_items: Vec::new(),
            global_menu_bar: true,
            current_frame: 0,
            theme: Theme::default(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        let mut colors = HashMap::new();
        colors.insert("WindowBg".to_string(), [0.06, 0.06, 0.06, 0.94]);
        colors.insert("TitleBg".to_string(), [0.04, 0.04, 0.04, 1.0]);
        Self {
            colors,
            style_vars: HashMap::new(),
            font_size: 13.0,
            rounding: 4.0,
        }
    }
}

// ========= SERVICE =========

// Rename the remote client to avoid conflict
cell_remote!(RenderClient = "renderer");

#[service]
#[derive(Clone)]
struct UIService {
    state: Arc<RwLock<UIState>>,
    renderer_client: Arc<Mutex<Option<RenderClient::Client>>>,
    event_queue: Arc<Mutex<Vec<WidgetEvent>>>,
}

#[handler]
impl UIService {
    async fn register_window(&self, req: RegisterWindow) -> Result<()> {
        let mut state = self.state.write();
        
        state.windows.insert(req.window_id.clone(), WindowState {
            id: req.window_id,
            title: req.title,
            visible: true,
            position: req.initial_position,
            size: req.initial_size,
            flags: req.flags,
            widgets: HashMap::new(),
            widget_states: HashMap::new(),
        });
        
        Ok(())
    }
    
    async fn unregister_window(&self, req: UnregisterWindow) -> Result<()> {
        let mut state = self.state.write();
        state.windows.remove(&req.window_id);
        Ok(())
    }
    
    async fn show_window(&self, req: ShowWindow) -> Result<()> {
        let mut state = self.state.write();
        if let Some(window) = state.windows.get_mut(&req.window_id) {
            window.visible = req.visible;
        }
        Ok(())
    }
    
    async fn add_menu_item(&self, req: AddMenuItem) -> Result<()> {
        let mut state = self.state.write();
        state.menu_items.push(MenuItem {
            path: req.menu_path,
            label: req.label,
            shortcut: req.shortcut,
            enabled: req.enabled,
            window_id: req.window_id,
        });
        Ok(())
    }
    
    async fn add_widget(&self, req: AddWidget) -> Result<()> {
        // Extract data before locking to avoid move issues
        let default_val = widget_default_value(&req.widget);
        let widget_id = get_widget_id(&req.widget);
        
        let mut state = self.state.write();
        let current_frame = state.current_frame;
        
        if let Some(window) = state.windows.get_mut(&req.window_id) {
            window.widgets.insert(widget_id.clone(), req.widget);
            
            if let Some(default) = default_val {
                window.widget_states.insert(widget_id, WidgetState {
                    value: default,
                    last_frame_active: current_frame,
                });
            }
        }
        Ok(())
    }
    
    async fn update_widget(&self, req: WidgetUpdate) -> Result<()> {
        let mut state = self.state.write();
        let current_frame = state.current_frame;
        
        if let Some(window) = state.windows.get_mut(&req.window_id) {
            if let Some(widget_state) = window.widget_states.get_mut(&req.widget_id) {
                widget_state.value = req.value;
                widget_state.last_frame_active = current_frame;
            }
        }
        Ok(())
    }
    
    async fn set_theme(&self, req: SetTheme) -> Result<()> {
        let mut state = self.state.write();
        
        // Apply to ImGui first using req.theme to avoid borrow conflict
        let style = state.imgui.0.style_mut();
        
        for (name, color) in &req.theme.colors {
            if let Some(col_idx) = color_name_to_style_color(name) {
                style.colors[col_idx as usize] = (*color).into();
            }
        }
        style.window_rounding = req.theme.rounding;

        // Then update state storage
        state.theme = req.theme;
        
        Ok(())
    }
    
    async fn get_events(&self) -> Result<Vec<WidgetEvent>> {
        let mut events = self.event_queue.lock().await;
        Ok(std::mem::take(&mut *events))
    }
    
    async fn render_ui(&self, _req: RenderUI) -> Result<UIFrame> {
        let mut state_guard = self.state.write();
        let state = &mut *state_guard; // Deref to &mut UIState to allow splitting borrows
        
        state.current_frame += 1;
        
        // Split borrows to satisfy borrow checker
        let imgui = &mut state.imgui;
        let windows = &mut state.windows;
        let menu_items = &state.menu_items;
        let global_menu_bar = state.global_menu_bar;
        let current_frame = state.current_frame;
        let event_queue = self.event_queue.clone();
        
        // Start ImGui frame
        let ui = imgui.0.frame();
        
        // Global Menu
        if global_menu_bar {
            if let Some(_token) = ui.begin_main_menu_bar() {
                Self::draw_menu_bar(&ui, menu_items);
            }
        }
        
        // Windows
        for window in windows.values_mut() {
            if window.visible {
                Self::draw_window(&ui, window, current_frame, &event_queue);
            }
        }
        
        // End frame (internal imgui state update)
        // Note: In a real app we'd use render() and process draw data.
        // Here we just return empty bytes to satisfy protocol since actual 
        // draw data structs are not serializable without custom code.
        
        Ok(UIFrame {
            draw_data: Vec::new(),
            width: 1280,
            height: 720,
        })
    }
}

// Logic implementations
impl UIService {
    fn draw_menu_bar(ui: &imgui::Ui, menu_items: &[MenuItem]) {
        let mut menus: HashMap<String, Vec<&MenuItem>> = HashMap::new();
        
        for item in menu_items {
            if item.window_id.is_none() {
                let parts: Vec<&str> = item.path.split('/').collect();
                if !parts.is_empty() {
                    menus.entry(parts[0].to_string()).or_default().push(item);
                }
            }
        }
        
        for (menu_name, items) in menus {
            if let Some(_token) = ui.begin_menu(&menu_name) {
                    for item in items {
                    let path_parts: Vec<&str> = item.path.split('/').collect();
                    let label_str = item.label.as_str();
                    let label = path_parts.last().copied().unwrap_or(label_str);
                    
                    if ui.menu_item_config(label)
                        .shortcut(item.shortcut.as_deref().unwrap_or(""))
                        .enabled(item.enabled)
                        .build()
                    {
                        // Events would go here
                    }
                }
            }
        }
    }
    
    fn draw_window(
        ui: &imgui::Ui, 
        window: &mut WindowState, 
        current_frame: u64,
        event_queue: &Arc<Mutex<Vec<WidgetEvent>>>
    ) {
        let window_name = format!("{}###{}", window.title, window.id);
        
        let mut flags = imgui::WindowFlags::empty();
        for flag in &window.flags {
            // Mapping essential flags
            use imgui::WindowFlags as F;
            match flag {
                WindowFlag::NoTitleBar => flags.insert(F::NO_TITLE_BAR),
                WindowFlag::NoResize => flags.insert(F::NO_RESIZE),
                WindowFlag::NoMove => flags.insert(F::NO_MOVE),
                WindowFlag::NoScrollbar => flags.insert(F::NO_SCROLLBAR),
                WindowFlag::NoCollapse => flags.insert(F::NO_COLLAPSE),
                WindowFlag::AlwaysAutoResize => flags.insert(F::ALWAYS_AUTO_RESIZE),
                WindowFlag::NoBackground => flags.insert(F::NO_BACKGROUND),
                WindowFlag::MenuBar => flags.insert(F::MENU_BAR),
                _ => {}
            }
        }
        
        let [px, py] = window.position.unwrap_or([50.0, 50.0]);
        let [sx, sy] = window.size.unwrap_or([300.0, 400.0]);
        
        // Setup window
        let token = ui.window(&window_name)
            .position([px, py], imgui::Condition::FirstUseEver)
            .size([sx, sy], imgui::Condition::FirstUseEver)
            .flags(flags)
            .begin();
            
        if let Some(_token) = token {
            // Collect IDs to avoid borrow conflict in loop
            let widget_ids: Vec<String> = window.widgets.keys().cloned().collect();
            
            for widget_id in widget_ids {
                if let Some(widget) = window.widgets.get(&widget_id).cloned() {
                    Self::draw_widget(ui, &widget, &widget_id, window, current_frame, event_queue);
                }
            }
        }
    }
    
    fn draw_widget(
        ui: &imgui::Ui, 
        widget: &Widget, 
        widget_id: &str, 
        window: &mut WindowState, 
        current_frame: u64,
        event_queue: &Arc<Mutex<Vec<WidgetEvent>>>
    ) {
        match widget {
            Widget::Button { label, size, .. } => {
                let clicked = if let Some([w, h]) = size {
                    ui.button_with_size(label, [*w, *h])
                } else {
                    ui.button(label)
                };
                
                if clicked {
                    let event = WidgetEvent {
                        window_id: window.id.clone(),
                        widget_id: widget_id.to_string(),
                        event_type: WidgetEventType::Clicked,
                        value: None,
                    };
                    let q = event_queue.clone();
                    tokio::spawn(async move { q.lock().await.push(event); });
                }
            }
            Widget::Checkbox { label, initial, .. } => {
                let current = window.widget_states.get(widget_id)
                    .and_then(|s| if let WidgetValue::Bool(b) = s.value { Some(b) } else { None })
                    .unwrap_or(*initial);
                
                let mut val = current;
                if ui.checkbox(label, &mut val) {
                    window.widget_states.insert(widget_id.to_string(), WidgetState {
                        value: WidgetValue::Bool(val),
                        last_frame_active: current_frame,
                    });
                    
                    let event = WidgetEvent {
                        window_id: window.id.clone(),
                        widget_id: widget_id.to_string(),
                        event_type: WidgetEventType::Changed,
                        value: Some(WidgetValue::Bool(val)),
                    };
                    let q = event_queue.clone();
                    tokio::spawn(async move { q.lock().await.push(event); });
                }
            }
            Widget::Text { content, .. } => {
                ui.text(content);
            }
            Widget::Separator => {
                ui.separator();
            }
            // Add other widgets as needed...
            _ => {
                ui.text("Widget not implemented");
            }
        }
    }
}

// Helpers

fn widget_default_value(widget: &Widget) -> Option<WidgetValue> {
    match widget {
        Widget::Checkbox { initial, .. } => Some(WidgetValue::Bool(*initial)),
        Widget::SliderFloat { initial, .. } => Some(WidgetValue::Float(*initial)),
        Widget::SliderInt { initial, .. } => Some(WidgetValue::Int(*initial)),
        Widget::InputText { initial, .. } => Some(WidgetValue::String(initial.clone())),
        Widget::ColorPicker { initial, .. } => Some(WidgetValue::Color(*initial)),
        Widget::ComboBox { initial, .. } => Some(WidgetValue::Selection(*initial)),
        Widget::ListBox { initial, .. } => Some(WidgetValue::MultiSelection(initial.clone())),
        _ => None,
    }
}

fn get_widget_id(widget: &Widget) -> String {
    match widget {
        Widget::Button { id, .. } => id.clone(),
        Widget::Checkbox { id, .. } => id.clone(),
        Widget::SliderFloat { id, .. } => id.clone(),
        Widget::SliderInt { id, .. } => id.clone(),
        Widget::InputText { id, .. } => id.clone(),
        Widget::ColorPicker { id, .. } => id.clone(),
        Widget::ComboBox { id, .. } => id.clone(),
        Widget::ListBox { id, .. } => id.clone(),
        Widget::Plot { id, .. } => id.clone(),
        Widget::ProgressBar { id, .. } => id.clone(),
        Widget::Text { id, .. } => id.clone(),
        Widget::Image { id, .. } => id.clone(),
        Widget::Separator => "separator".to_string(),
    }
}

fn color_name_to_style_color(name: &str) -> Option<imgui::StyleColor> {
    use imgui::StyleColor::*;
    match name {
        "Text" => Some(Text),
        "WindowBg" => Some(WindowBg),
        "Border" => Some(Border),
        "TitleBg" => Some(TitleBg),
        "Button" => Some(Button),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🎨 UI Cell - Dear ImGui Layout Engine");
    
    let service = UIService {
        state: Arc::new(RwLock::new(UIState::new())),
        renderer_client: Arc::new(Mutex::new(None)),
        event_queue: Arc::new(Mutex::new(Vec::new())),
    };
    
    service.serve("ui").await
}