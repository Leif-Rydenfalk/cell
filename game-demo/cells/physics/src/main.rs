//! Physics Cell - 2D Rigid Body Engine
//!
//! This cell owns the Rapier2D physics world and provides:
//! - Deterministic physics simulation at 60Hz
//! - RPC API for spawning bodies, applying forces
//! - Transform snapshots for renderer
//! - Collision events

use anyhow::Result;
use cell_sdk::*;
use rapier2d::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use std::time::Duration;

// ========= PROTEINS (PUBLIC API) =========

#[protein]
pub struct RigidBodyDef {
    pub id: u64,
    pub shape: Shape,
    pub position: [f32; 2],
    pub rotation: f32,
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    pub is_static: bool,
    pub collider_groups: Option<[u32; 2]>,
}

#[protein]
pub enum Shape {
    Cuboid { hx: f32, hy: f32 },
    Ball { radius: f32 },
    Capsule { hx: f32, hy: f32 },
    Polygon { vertices: Vec<[f32; 2]> },
}

#[protein]
pub struct SpawnBody {
    pub def: RigidBodyDef,
}

#[protein]
pub struct SpawnBodyResponse {
    pub handle: u64,
}

#[protein]
pub struct DespawnBody {
    pub id: u64,
}

#[protein]
pub struct ApplyForce {
    pub body_id: u64,
    pub force: [f32; 2],
    pub point: Option<[f32; 2]>,
}

#[protein]
pub struct ApplyImpulse {
    pub body_id: u64,
    pub impulse: [f32; 2],
    pub point: Option<[f32; 2]>,
}

#[protein]
pub struct ApplyTorque {
    pub body_id: u64,
    pub torque: f32,
}

#[protein]
pub struct SetVelocity {
    pub body_id: u64,
    pub linvel: [f32; 2],
    pub angvel: f32,
}

#[protein]
pub struct GetTransform {
    pub body_id: u64,
}

#[protein]
pub struct Transform {
    pub position: [f32; 2],
    pub rotation: f32,
}

#[protein]
pub struct GetSnapshot {
    pub include_colliders: bool,
}

// This is the physics snapshot - note the name to avoid confusion with World's snapshot
#[protein]
pub struct PhysicsSnapshot {
    pub timestamp: u64,
    pub bodies: Vec<BodySnapshot>,
}

#[protein]
pub struct BodySnapshot {
    pub id: u64,
    pub position: [f32; 2],
    pub rotation: f32,
    pub linvel: [f32; 2],
    pub angvel: f32,
    pub colliders: Vec<ColliderSnapshot>,
}

#[protein]
pub struct ColliderSnapshot {
    pub shape: Shape,
    pub position: [f32; 2],
    pub rotation: f32,
}

#[protein]
pub struct RayCast {
    pub origin: [f32; 2],
    pub direction: [f32; 2],
    pub max_toi: f32,
    pub groups: Option<[u32; 2]>,
}

#[protein]
pub struct RayCastHit {
    pub body_id: u64,
    pub collider_id: u64,
    pub point: [f32; 2],
    pub normal: [f32; 2],
    pub toi: f32,
}

#[protein]
pub struct SetGravity {
    pub gravity: [f32; 2],
}

#[protein]
pub struct GetGravity;

#[protein]
pub struct QueryAABB {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

// Ping/Pong for health checks
#[protein]
pub struct Ping;
#[protein]
pub struct Pong {
    pub timestamp: u64,
}

// ========= INTERNAL STATE =========

struct PhysicsState {
    gravity: Vector<f32>,
    integration_parameters: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: BroadPhase, // Fixed: DefaultBroadPhase -> BroadPhase
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,
    body_map: HashMap<u64, RigidBodyHandle>,
    collider_map: HashMap<u64, ColliderHandle>,
    next_id: u64,
    frame_count: u64,
}

impl PhysicsState {
    fn new() -> Self {
        Self {
            gravity: vector![0.0, -9.81],
            integration_parameters: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: BroadPhase::new(), // Fixed: DefaultBroadPhase::new() -> BroadPhase::new()
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            body_map: HashMap::new(),
            collider_map: HashMap::new(),
            next_id: 1,
            frame_count: 0,
        }
    }

    fn step(&mut self, _dt: f32) {
        let event_handler = ();
        
        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &(),
            &event_handler,
        );
        
        self.frame_count += 1;
    }

    fn spawn_body(&mut self, def: RigidBodyDef) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        
        let mut rb_builder = RigidBodyBuilder::new(
            if def.is_static { RigidBodyType::Fixed } else { RigidBodyType::Dynamic }
        )
        .position(Isometry::new(
            vector![def.position[0], def.position[1]],
            def.rotation,
        ))
        .user_data(id as u128);
        
        if !def.is_static {
            rb_builder = rb_builder
                .linear_damping(0.5)
                .angular_damping(0.5);
        }
        
        let rigid_body = rb_builder.build();
        let rb_handle = self.bodies.insert(rigid_body);
        self.body_map.insert(id, rb_handle);
        
        let collider_builder = match def.shape {
            Shape::Cuboid { hx, hy } => ColliderBuilder::cuboid(hx, hy),
            Shape::Ball { radius } => ColliderBuilder::ball(radius),
            Shape::Capsule { hx, hy } => {
                let half_height = hy - hx;
                ColliderBuilder::capsule_y(half_height, hx)
            }
            Shape::Polygon { vertices } => {
                let points: Vec<Point<f32>> = vertices
                    .iter()
                    .map(|v| point![v[0], v[1]])
                    .collect();
                ColliderBuilder::convex_hull(&points).unwrap_or_else(|| ColliderBuilder::ball(0.5))
            }
        };
        
        let collider = collider_builder
            .density(def.density)
            .friction(def.friction)
            .restitution(def.restitution)
            .user_data(id as u128)
            .build();
        
        let co_handle = self.colliders.insert_with_parent(
            collider,
            rb_handle,
            &mut self.bodies,
        );
        
        self.collider_map.insert(id, co_handle);
        
        id
    }

    fn despawn_body(&mut self, id: u64) {
        if let Some(rb_handle) = self.body_map.remove(&id) {
            self.bodies.remove(
                rb_handle,
                &mut self.island_manager,
                &mut self.colliders,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                true,
            );
        }
        self.collider_map.remove(&id);
    }

    fn apply_force(&mut self, body_id: u64, force: [f32; 2], point: Option<[f32; 2]>) {
        if let Some(&rb_handle) = self.body_map.get(&body_id) {
            if let Some(body) = self.bodies.get_mut(rb_handle) {
                if let Some(p) = point {
                    body.add_force_at_point(
                        vector![force[0], force[1]],
                        point![p[0], p[1]],
                        true,
                    );
                } else {
                    body.add_force(vector![force[0], force[1]], true);
                }
            }
        }
    }

    fn get_snapshot(&self) -> PhysicsSnapshot {
        let mut bodies = Vec::new();
        
        for (&id, &rb_handle) in &self.body_map {
            if let Some(body) = self.bodies.get(rb_handle) {
                let pos = body.position().translation.vector;
                let rot = body.position().rotation.angle();
                
                let mut colliders = Vec::new();
                for (_, collider) in self.colliders.iter() { // Fixed: removed unused co_handle
                    if collider.parent() == Some(rb_handle) {
                        let co_pos = collider.position().translation.vector;
                        let co_rot = collider.position().rotation.angle();
                        
                        colliders.push(ColliderSnapshot {
                            shape: shape_to_protein(collider.shape()),
                            position: [co_pos.x, co_pos.y],
                            rotation: co_rot,
                        });
                    }
                }
                
                bodies.push(BodySnapshot {
                    id,
                    position: [pos.x, pos.y],
                    rotation: rot,
                    linvel: [body.linvel().x, body.linvel().y],
                    angvel: body.angvel(),
                    colliders,
                });
            }
        }
        
        PhysicsSnapshot {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            bodies,
        }
    }
}

fn shape_to_protein(shape: &dyn rapier2d::geometry::Shape) -> Shape {
    if let Some(cuboid) = shape.as_cuboid() {
        Shape::Cuboid {
            hx: cuboid.half_extents.x,
            hy: cuboid.half_extents.y,
        }
    } else if let Some(ball) = shape.as_ball() {
        Shape::Ball { radius: ball.radius }
    } else if let Some(capsule) = shape.as_capsule() {
        Shape::Capsule {
            hx: capsule.radius,
            hy: capsule.half_height(),
        }
    } else {
        Shape::Cuboid { hx: 0.5, hy: 0.5 }
    }
}

// ========= SERVICE =========

#[service]
#[derive(Clone)]
struct PhysicsService {
    state: Arc<RwLock<PhysicsState>>,
    world_client: Arc<Mutex<Option<World::Client>>>,
}

cell_remote!(World = "world");

#[handler]
impl PhysicsService {
    async fn ping(&self, _req: Ping) -> Result<Pong> {
        Ok(Pong {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        })
    }
    
    async fn spawn_body(&self, req: SpawnBody) -> Result<SpawnBodyResponse> {
        let mut state = self.state.write().await;
        let handle = state.spawn_body(req.def);
        
        if let Ok(world) = self.ensure_world().await {
            let _ = world.on_body_spawned(World::OnBodySpawned {
                body_id: handle,
            }).await;
        }
        
        Ok(SpawnBodyResponse { handle })
    }
    
    async fn despawn_body(&self, req: DespawnBody) -> Result<()> {
        let mut state = self.state.write().await;
        state.despawn_body(req.id);
        
        if let Ok(world) = self.ensure_world().await {
            let _ = world.on_body_despawned(World::OnBodyDespawned {
                body_id: req.id,
            }).await;
        }
        
        Ok(())
    }
    
    async fn apply_force(&self, req: ApplyForce) -> Result<()> {
        let mut state = self.state.write().await;
        state.apply_force(req.body_id, req.force, req.point);
        Ok(())
    }
    
    async fn apply_impulse(&self, req: ApplyImpulse) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(&rb_handle) = state.body_map.get(&req.body_id) {
            if let Some(body) = state.bodies.get_mut(rb_handle) {
                if let Some(p) = req.point {
                    body.apply_impulse_at_point(
                        vector![req.impulse[0], req.impulse[1]],
                        point![p[0], p[1]],
                        true,
                    );
                } else {
                    body.apply_impulse(vector![req.impulse[0], req.impulse[1]], true);
                }
            }
        }
        Ok(())
    }
    
    async fn apply_torque(&self, req: ApplyTorque) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(&rb_handle) = state.body_map.get(&req.body_id) {
            if let Some(body) = state.bodies.get_mut(rb_handle) {
                // Fixed: apply_torque_impulse instead of apply_torque
                body.apply_torque_impulse(req.torque, true);
            }
        }
        Ok(())
    }
    
    async fn set_velocity(&self, req: SetVelocity) -> Result<()> {
        let mut state = self.state.write().await;
        if let Some(&rb_handle) = state.body_map.get(&req.body_id) {
            if let Some(body) = state.bodies.get_mut(rb_handle) {
                body.set_linvel(vector![req.linvel[0], req.linvel[1]], true);
                body.set_angvel(req.angvel, true);
            }
        }
        Ok(())
    }
    
    async fn get_transform(&self, req: GetTransform) -> Result<Option<Transform>> {
        let state = self.state.read().await;
        if let Some(&rb_handle) = state.body_map.get(&req.body_id) {
            if let Some(body) = state.bodies.get(rb_handle) {
                let pos = body.position().translation.vector;
                let rot = body.position().rotation.angle();
                return Ok(Some(Transform {
                    position: [pos.x, pos.y],
                    rotation: rot,
                }));
            }
        }
        Ok(None)
    }
    
    async fn get_snapshot(&self, _req: GetSnapshot) -> Result<PhysicsSnapshot> {
        let state = self.state.read().await;
        Ok(state.get_snapshot())
    }
    
    async fn ray_cast(&self, req: RayCast) -> Result<Vec<RayCastHit>> {
        let state = self.state.read().await;
        
        let filter = QueryFilter::default();
        
        let ray = Ray::new(
            point![req.origin[0], req.origin[1]],
            vector![req.direction[0], req.direction[1]],
        );
        
        let mut hits = Vec::new();
        
        state.query_pipeline.intersections_with_ray(
            &state.bodies,
            &state.colliders,
            &ray,
            req.max_toi,
            true,
            filter,
            |handle, intersection| {
                if let Some(collider) = state.colliders.get(handle) {
                    if let Some(body_handle) = collider.parent() {
                        if let Some(body) = state.bodies.get(body_handle) {
                            let body_id = body.user_data as u64;
                            // Fixed: intersection fields are toi, normal, feature (no point field)
                            let point = ray.point_at(intersection.toi);
                            hits.push(RayCastHit {
                                body_id,
                                collider_id: collider.user_data as u64,
                                point: [point.x, point.y],
                                normal: [intersection.normal.x, intersection.normal.y],
                                toi: intersection.toi, // Fixed: time_of_impact -> toi
                            });
                        }
                    }
                }
                true
            },
        );
        
        Ok(hits)
    }
    
    async fn set_gravity(&self, req: SetGravity) -> Result<()> {
        let mut state = self.state.write().await;
        state.gravity = vector![req.gravity[0], req.gravity[1]];
        Ok(())
    }
    
    async fn get_gravity(&self, _req: GetGravity) -> Result<[f32; 2]> {
        let state = self.state.read().await;
        Ok([state.gravity.x, state.gravity.y])
    }
    
    async fn query_aabb(&self, req: QueryAABB) -> Result<Vec<u64>> {
        let state = self.state.read().await;
        
        let aabb = Aabb::new(
            point![req.min[0], req.min[1]],
            point![req.max[0], req.max[1]],
        );
        
        let mut result = Vec::new();
        
        state.query_pipeline.colliders_with_aabb_intersecting_aabb(
            &aabb,
            |handle| {
                if let Some(collider) = state.colliders.get(*handle) {
                    if let Some(parent) = collider.parent() {
                        if let Some(body) = state.bodies.get(parent) {
                            result.push(body.user_data as u64);
                        }
                    }
                }
                true
            },
        );
        
        Ok(result)
    }
}

impl PhysicsService {
    async fn ensure_world(&self) -> Result<World::Client> {
        let mut guard = self.world_client.lock().await;
        
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match World::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Physics] Connected to World");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Failed to connect to World: {}", e)),
        }
    }
    
    async fn tick_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_micros(16667));
        
        loop {
            interval.tick().await;
            
            {
                let mut state = self.state.write().await;
                state.step(1.0 / 60.0);
            }
            
            if let Ok(world) = self.ensure_world().await {
                let snapshot = {
                    let state = self.state.read().await;
                    state.get_snapshot()
                };
                
                // Fixed: Convert our PhysicsSnapshot to World::PhysicsSnapshot
                // This is a workaround - ideally you'd have a common type or conversion
                let world_snapshot = World::PhysicsSnapshot {
                    timestamp: snapshot.timestamp,
                    bodies: snapshot.bodies.into_iter().map(|b| World::PhysicsBodySnapshot {
                        id: b.id,
                        position: b.position,
                        rotation: b.rotation,
                        linvel: b.linvel,
                        angvel: b.angvel,
                    }).collect(),
                };
                
                let _ = world.on_physics_tick(World::OnPhysicsTick {
                    snapshot: world_snapshot,
                }).await;
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🔄 Physics Cell - 2D Rigid Body Engine");
    println!("   └─ Rapier2D physics @ 60Hz");
    println!("   └─ Bodies: 0");
    println!("   └─ Gravity: (0.0, -9.81)");
    
    let service = PhysicsService {
        state: Arc::new(RwLock::new(PhysicsState::new())),
        world_client: Arc::new(Mutex::new(None)),
    };
    
    let tick_service = service.clone();
    tokio::spawn(async move {
        tick_service.tick_loop().await;
    });
    
    service.serve("physics").await
}