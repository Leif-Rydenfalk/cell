use crate::KeyCode; // Imported from main which defines it
use std::collections::HashSet;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton};
use winit::keyboard::KeyCode as WinitKeyCode;

pub struct Input {
    keys_down: HashSet<KeyCode>,
    mouse_buttons_down: HashSet<MouseButton>,
    mouse_position: (f64, f64),
    mouse_delta: (f64, f64),
    scroll_delta: f64,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            keys_down: HashSet::new(),
            mouse_buttons_down: HashSet::new(),
            mouse_position: (0.0, 0.0),
            mouse_delta: (0.0, 0.0),
            scroll_delta: 0.0,
        }
    }
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_key_input(&mut self, key: KeyCode, state: ElementState) {
        match state {
            ElementState::Pressed => {
                self.keys_down.insert(key);
            }
            ElementState::Released => {
                self.keys_down.remove(&key);
            }
        }
    }

    pub fn handle_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        match state {
            ElementState::Pressed => {
                self.mouse_buttons_down.insert(button);
            }
            ElementState::Released => {
                self.mouse_buttons_down.remove(&button);
            }
        }
    }

    pub fn handle_cursor_moved(&mut self, position: &PhysicalPosition<f64>) {
        self.mouse_position = (position.x, position.y);
    }

    pub fn handle_mouse_motion(&mut self, delta: (f64, f64)) {
        self.mouse_delta.0 += delta.0;
        self.mouse_delta.1 += delta.1;
    }

    pub fn handle_mouse_scroll(&mut self, delta: f64) {
        self.scroll_delta += delta;
    }

    pub fn get_and_reset_state(&mut self) -> (Vec<u16>, [f32; 2]) {
        // We return Vec<u16> to match the shared state logic which stores u16
        // KeyCode repr is u16 so cast is safe.
        // Removed unnecessary unsafe block.
        let keys: Vec<u16> = self.keys_down.iter().map(|k| *k as u16).collect();
        let delta = [self.mouse_delta.0 as f32, self.mouse_delta.1 as f32];
        self.mouse_delta = (0.0, 0.0);
        self.scroll_delta = 0.0;
        (keys, delta)
    }
}

pub fn map_to_u16(k: WinitKeyCode) -> KeyCode {
    match k {
        WinitKeyCode::KeyW => KeyCode::W,
        WinitKeyCode::KeyA => KeyCode::A,
        WinitKeyCode::KeyS => KeyCode::S,
        WinitKeyCode::KeyD => KeyCode::D,
        WinitKeyCode::KeyQ => KeyCode::Q,
        WinitKeyCode::KeyE => KeyCode::E,
        WinitKeyCode::Space => KeyCode::Space,
        WinitKeyCode::ShiftLeft | WinitKeyCode::ShiftRight => KeyCode::Shift,
        WinitKeyCode::Escape => KeyCode::Esc,
        _ => KeyCode::Unknown,
    }
}
