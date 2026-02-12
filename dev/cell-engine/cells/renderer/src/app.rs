use crate::wgpu_ctx::WgpuCtx;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, Event, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

#[derive(Default)]
pub struct App<'window> {
    window: Option<Arc<Window>>,
    ctx: Option<WgpuCtx<'window>>,
}

impl<'window> ApplicationHandler for App<'window> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let win_attr = Window::default_attributes().with_title("Cell Retina");
            let window = Arc::new(event_loop.create_window(win_attr).unwrap());
            // Lock cursor for RPG feel
            window.set_cursor_visible(false);
            // window.set_cursor_grab(winit::window::CursorGrabMode::Locked).ok(); // Optional

            let ctx = pollster::block_on(WgpuCtx::new_async(window.clone()));

            self.window = Some(window);
            self.ctx = Some(ctx);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(ctx) = self.ctx.as_mut() {
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(size) => {
                    ctx.resize((size.width, size.height));
                }
                WindowEvent::RedrawRequested => {
                    ctx.draw();
                    // Request next frame immediately (Uncapped FPS)
                    if let Some(win) = self.window.as_ref() {
                        win.request_redraw();
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        ctx.input.handle_key_input(code, event.state);
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    ctx.input.handle_cursor_moved(&position);
                }
                _ => {}
            }
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let Some(ctx) = self.ctx.as_mut() {
            match event {
                DeviceEvent::MouseMotion { delta } => {
                    ctx.input.handle_mouse_motion(delta);
                }
                _ => {}
            }
        }
    }
}
