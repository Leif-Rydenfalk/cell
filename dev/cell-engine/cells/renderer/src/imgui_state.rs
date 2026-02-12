use std::time::Instant;

use imgui::*;
use imgui_wgpu::Renderer;
use imgui_winit_support::WinitPlatform;

pub struct ImguiState {
    pub context: imgui::Context,
    pub platform: WinitPlatform,
    pub renderer: Renderer,
    pub clear_color: wgpu::Color,
    pub demo_open: bool,
    pub last_frame: Instant,
    pub last_cursor: Option<MouseCursor>,
}
