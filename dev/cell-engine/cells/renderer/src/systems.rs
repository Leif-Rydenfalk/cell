use crate::*;
use cgmath::{perspective, InnerSpace, Matrix4, Point3, Quaternion, Rad, Rotation3, Vector3, Zero};
use hecs::World;
use std::time::Duration;

#[derive(Debug)]
pub struct Transform {
    pub position: Point3<f32>,
    pub rotation: Quaternion<f32>,
    pub scale: Vector3<f32>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Point3::new(0.0, 0.0, 0.0),
            rotation: Quaternion::from_axis_angle(Vector3::unit_y(), Rad(0.0)),
            scale: Vector3::new(1.0, 1.0, 1.0),
        }
    }
}

#[derive(Debug)]
pub struct Camera {
    pub fov: Rad<f32>,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
    pub up_vector: Vector3<f32>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov: Rad(std::f32::consts::FRAC_PI_4),
            aspect: 16.0 / 9.0,
            near: 0.1,
            far: 1000.0,
            up_vector: Vector3::unit_y(),
        }
    }
}

#[derive(Debug)]
pub struct CameraController {
    pub move_speed: f32,
    pub look_speed: f32,
    pub pitch: Rad<f32>,
    pub yaw: Rad<f32>,
    pub pitch_limit: Rad<f32>,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            move_speed: 10.0,
            look_speed: 0.003,
            pitch: Rad(0.0),
            yaw: Rad(0.0),
            pitch_limit: Rad(std::f32::consts::FRAC_PI_2 - 0.1),
        }
    }
}

pub fn update_camera_system(world: &mut World, input: &Input, dt: Duration, is_active: bool) {
    for (_, (transform, _camera, controller)) in
        world.query_mut::<(&mut Transform, &Camera, &mut CameraController)>()
    {
        if !is_active {
            continue;
        }

        let dt = dt.as_secs_f32();

        if input.scroll_delta() != 0.0 {
            let scroll_factor = if input.scroll_delta() > 0.0 { 1.2 } else { 0.8 };
            controller.move_speed *= scroll_factor;
            controller.move_speed = controller.move_speed.clamp(0.01, 1000.0);
        }

        let (dx, dy) = input.mouse_delta();
        if dx != 0.0 || dy != 0.0 {
            controller.yaw -= Rad(dx as f32 * controller.look_speed);
            controller.pitch -= Rad(dy as f32 * controller.look_speed);
            controller.pitch = Rad(controller
                .pitch
                .0
                .clamp(-controller.pitch_limit.0, controller.pitch_limit.0));
        }

        transform.rotation = Quaternion::from_axis_angle(Vector3::unit_y(), controller.yaw)
            * Quaternion::from_axis_angle(Vector3::unit_x(), controller.pitch);

        let forward = transform.rotation * -Vector3::unit_z();
        let right = transform.rotation * Vector3::unit_x();

        let mut movement_input = Vector3::zero();
        if input.is_key_down(winit::keyboard::KeyCode::KeyW) {
            movement_input += forward;
        }
        if input.is_key_down(winit::keyboard::KeyCode::KeyS) {
            movement_input -= forward;
        }
        if input.is_key_down(winit::keyboard::KeyCode::KeyA) {
            movement_input -= right;
        }
        if input.is_key_down(winit::keyboard::KeyCode::KeyD) {
            movement_input += right;
        }
        if input.is_key_down(winit::keyboard::KeyCode::KeyE) {
            movement_input += Vector3::unit_y();
        }
        if input.is_key_down(winit::keyboard::KeyCode::KeyQ) {
            movement_input -= Vector3::unit_y();
        }

        // REMOVED: broken wgpu_ctx references.
        // Logic for debug printing should be moved to the App struct where wgpu_ctx is accessible,
        // or passed via a channel/event.

        if movement_input != Vector3::zero() {
            let movement = movement_input.normalize() * controller.move_speed * dt;
            transform.position += movement;
        }
    }
}

pub fn calculate_view_matrix(transform: &Transform) -> Matrix4<f32> {
    let position = transform.position;
    let forward = transform.rotation * -Vector3::unit_z();
    let up = transform.rotation * Vector3::unit_y();
    let target = position + forward;
    Matrix4::look_at_rh(position, target, up)
}

pub fn calculate_view_projection(transform: &Transform, camera: &Camera) -> Matrix4<f32> {
    let view = calculate_view_matrix(transform);
    let proj = perspective(camera.fov, camera.aspect, camera.near, camera.far);
    proj * view
}
