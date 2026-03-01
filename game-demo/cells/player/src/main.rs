//! Player Cell - Input & Camera Controller
//!
//! This cell provides:
//! - Ship control (thrust, rotation, mining)
//! - Camera following
//! - UI interactions
//! - Inventory management UI

use anyhow::Result;
use cell_sdk::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

// ========= PROTEINS =========

#[protein]
pub struct PlayerState {
    pub player_id: u64,
    pub name: String,
    pub ship_id: Option<u64>,
    pub position: [f32; 2],
    pub rotation: f32,
    pub velocity: [f32; 2],
    pub camera_position: [f32; 2],
    pub camera_zoom: f32,
    pub selected_ship_component: Option<String>,
    pub mining_target: Option<u64>,
    pub inventory_open: bool,
    pub factory_open: bool,
}

#[protein]
pub struct InputState {
    pub thrust_forward: f32,
    pub thrust_backward: f32,
    pub turn_left: f32,
    pub turn_right: f32,
    pub strafe_left: f32,
    pub strafe_right: f32,
    pub mine: bool,
    pub build_mode: bool,
    pub inventory_toggle: bool,
    pub factory_toggle: bool,
    pub mouse_position: [f32; 2],
    pub mouse_world_position: [f32; 2],
    pub mouse_click: bool,
}

#[protein]
pub struct SpawnPlayer {
    pub player_id: u64,
    pub name: String,
    pub spawn_position: [f32; 2],
}

#[protein]
pub struct ControlShip {
    pub player_id: u64,
    pub input: InputState,
}

#[protein]
pub struct GetPlayerState {
    pub player_id: u64,
}

// Simple ping/pong for health checks
#[protein]
pub struct Ping;
#[protein]
pub struct Pong;

// ========= SERVICE =========

#[service]
#[derive(Clone)]
struct PlayerService {
    players: Arc<RwLock<HashMap<u64, PlayerState>>>,
    renderer_client: Arc<Mutex<Option<Renderer::Client>>>,
    physics_client: Arc<Mutex<Option<Physics::Client>>>,
    world_client: Arc<Mutex<Option<World::Client>>>,
    inventory_client: Arc<Mutex<Option<Inventory::Client>>>,
    factory_client: Arc<Mutex<Option<Factory::Client>>>,
    ui_client: Arc<Mutex<Option<UI::Client>>>,
    asteroid_client: Arc<Mutex<Option<Asteroid::Client>>>,
}

cell_remote!(Renderer = "renderer");
cell_remote!(Physics = "physics");
cell_remote!(World = "world");
cell_remote!(Inventory = "inventory");
cell_remote!(Factory = "factory");
cell_remote!(UI = "ui");
cell_remote!(Asteroid = "asteroid");

#[handler]
impl PlayerService {
    async fn ping(&self, _req: Ping) -> Result<Pong> {
        Ok(Pong)
    }
    
    async fn spawn_player(&self, req: SpawnPlayer) -> Result<PlayerState> {
        let mut players = self.players.write().await;
        
        // Create player ship
        let ship_blueprint = Factory::CreateBlueprint {
            name: format!("{}'s Ship", req.name),
            description: "Player's mining vessel".to_string(),
            owner_id: req.player_id,
            width: 5,
            height: 5,
        };
        
        let blueprint = if let Ok(factory) = self.ensure_factory().await {
            factory.create_blueprint(ship_blueprint).await?
        } else {
            // Fallback - create basic blueprint structure
            Factory::ShipBlueprint {
                id: format!("bp_{}", req.player_id),
                name: format!("{}'s Ship", req.name),
                description: "Player's mining vessel".to_string(),
                owner_id: req.player_id,
                width: 5,
                height: 5,
                components: vec![
                    Factory::PlacedComponent {
                        component_id: "hull_basic".to_string(),
                        grid_x: 2,
                        grid_y: 2,
                        rotation: 0,
                        flipped: false,
                    },
                    Factory::PlacedComponent {
                        component_id: "thruster_small".to_string(),
                        grid_x: 2,
                        grid_y: 4,
                        rotation: 0,
                        flipped: false,
                    },
                    Factory::PlacedComponent {
                        component_id: "cargo_small".to_string(),
                        grid_x: 0,
                        grid_y: 2,
                        rotation: 0,
                        flipped: false,
                    },
                    Factory::PlacedComponent {
                        component_id: "drill_basic".to_string(),
                        grid_x: 4,
                        grid_y: 2,
                        rotation: 0,
                        flipped: false,
                    },
                ],
                stats: Factory::ShipStats {
                    total_mass: 0.0,
                    total_thrust: 0.0,
                    acceleration: 0.0,
                    rotation_speed: 3.0,
                    cargo_capacity: 100,
                    power_generation: 0,
                    power_consumption: 30,
                    shield_capacity: 0,
                    hull_integrity: 300,
                    drill_power: 50,
                    scanner_range: 100.0,
                },
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                version: 1,
            }
        };
        
        // Assemble ship
        let ship_id = if let Ok(factory) = self.ensure_factory().await {
            let assembly = factory.assemble_ship(Factory::AssembleShip {
                blueprint_id: blueprint.id,
                player_id: req.player_id,
                spawn_position: req.spawn_position,
            }).await?;
            
            Some(assembly.ship_entity_id)
        } else {
            // Fallback - spawn directly in physics
            if let Ok(physics) = self.ensure_physics().await {
                let def = Physics::RigidBodyDef {
                    id: 0,
                    shape: Physics::Shape::Cuboid { hx: 1.0, hy: 1.0 },
                    position: req.spawn_position,
                    rotation: 0.0,
                    density: 5.0,
                    friction: 0.5,
                    restitution: 0.2,
                    is_static: false,
                    collider_groups: None,
                };
                
                let resp = physics.spawn_body(Physics::SpawnBody { def }).await?;
                Some(resp.handle)
            } else {
                None
            }
        };
        
        // Join world
        if let Ok(world) = self.ensure_world().await {
            let _ = world.join_game(World::JoinGame {
                player_id: req.player_id,
                name: req.name.clone(),
            }).await;
        }
        
        // UI is currently a stub - skip UI operations for now
        // Just log that we would set up UI
        tracing::info!("[Player] Would set up UI for player {}", req.player_id);
        
        let state = PlayerState {
            player_id: req.player_id,
            name: req.name,
            ship_id,
            position: req.spawn_position,
            rotation: 0.0,
            velocity: [0.0, 0.0],
            camera_position: req.spawn_position,
            camera_zoom: 1.0,
            selected_ship_component: None,
            mining_target: None,
            inventory_open: false,
            factory_open: false,
        };
        
        players.insert(req.player_id, state.clone());
        
        Ok(state)
    }
    
    async fn control_ship(&self, req: ControlShip) -> Result<()> {
        let mut players = self.players.write().await;
        
        if let Some(player) = players.get_mut(&req.player_id) {
            if let Some(ship_id) = player.ship_id {
                if let Ok(physics) = self.ensure_physics().await {
                    // Apply thrust
                    let thrust_force = 100.0;
                    
                    if req.input.thrust_forward > 0.0 {
                        let force = [
                            player.rotation.cos() * thrust_force * req.input.thrust_forward,
                            player.rotation.sin() * thrust_force * req.input.thrust_forward,
                        ];
                        let _ = physics.apply_force(Physics::ApplyForce {
                            body_id: ship_id,
                            force,
                            point: None,
                        }).await;
                    }
                    
                    if req.input.thrust_backward > 0.0 {
                        let force = [
                            -player.rotation.cos() * thrust_force * req.input.thrust_backward,
                            -player.rotation.sin() * thrust_force * req.input.thrust_backward,
                        ];
                        let _ = physics.apply_force(Physics::ApplyForce {
                            body_id: ship_id,
                            force,
                            point: None,
                        }).await;
                    }
                    
                    if req.input.strafe_left > 0.0 {
                        let force = [
                            (player.rotation + std::f32::consts::FRAC_PI_2).cos() * thrust_force * 0.5 * req.input.strafe_left,
                            (player.rotation + std::f32::consts::FRAC_PI_2).sin() * thrust_force * 0.5 * req.input.strafe_left,
                        ];
                        let _ = physics.apply_force(Physics::ApplyForce {
                            body_id: ship_id,
                            force,
                            point: None,
                        }).await;
                    }
                    
                    if req.input.strafe_right > 0.0 {
                        let force = [
                            (player.rotation - std::f32::consts::FRAC_PI_2).cos() * thrust_force * 0.5 * req.input.strafe_right,
                            (player.rotation - std::f32::consts::FRAC_PI_2).sin() * thrust_force * 0.5 * req.input.strafe_right,
                        ];
                        let _ = physics.apply_force(Physics::ApplyForce {
                            body_id: ship_id,
                            force,
                            point: None,
                        }).await;
                    }
                    
                    // Apply torque
                    if req.input.turn_left > 0.0 {
                        let _ = physics.apply_torque(Physics::ApplyTorque {
                            body_id: ship_id,
                            torque: 10.0 * req.input.turn_left,
                        }).await;
                    }
                    
                    if req.input.turn_right > 0.0 {
                        let _ = physics.apply_torque(Physics::ApplyTorque {
                            body_id: ship_id,
                            torque: -10.0 * req.input.turn_right,
                        }).await;
                    }
                    
                    // Get updated transform
                    if let Some(transform) = physics.get_transform(Physics::GetTransform { body_id: ship_id }).await? {
                        player.position = transform.position;
                        player.rotation = transform.rotation;
                    }
                }
                
                // Mining
                if req.input.mine {
                    if let Some(target_id) = player.mining_target {
                        if let Ok(asteroid) = self.ensure_asteroid().await {
                            // TODO: Get drill power from ship components
                            let _ = asteroid.mine_asteroid(Asteroid::MineAsteroid {
                                field_id: "field_1".to_string(), // TODO: Track current field
                                asteroid_id: target_id,
                                drill_power: 50,
                                player_id: req.player_id,
                            }).await;
                        }
                    } else {
                        // Raycast for asteroids
                        if let Ok(physics) = self.ensure_physics().await {
                            let hits = physics.ray_cast(Physics::RayCast {
                                origin: player.position,
                                direction: [player.rotation.cos(), player.rotation.sin()],
                                max_toi: 50.0,
                                groups: Some([1, 1]), // Asteroid group
                            }).await?;
                            
                            if let Some(hit) = hits.first() {
                                player.mining_target = Some(hit.body_id);
                            }
                        }
                    }
                } else {
                    player.mining_target = None;
                }
                
                // Update camera to follow ship
                player.camera_position = player.position;
                
                // UI is currently a stub - skip toggle operations for now
                // Just update state
                if req.input.inventory_toggle {
                    player.inventory_open = !player.inventory_open;
                    tracing::debug!("[Player] Inventory toggled: {}", player.inventory_open);
                }
                
                if req.input.factory_toggle {
                    player.factory_open = !player.factory_open;
                    tracing::debug!("[Player] Factory toggled: {}", player.factory_open);
                }
            }
        }
        
        Ok(())
    }
    
    async fn get_player_state(&self, req: GetPlayerState) -> Result<Option<PlayerState>> {
        let players = self.players.read().await;
        Ok(players.get(&req.player_id).cloned())
    }
}

impl PlayerService {
    async fn ensure_renderer(&self) -> Result<Renderer::Client> {
        let mut guard = self.renderer_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Renderer::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Player] Connected to Renderer");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Renderer not available: {}", e)),
        }
    }
    
    async fn ensure_physics(&self) -> Result<Physics::Client> {
        let mut guard = self.physics_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Physics::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Player] Connected to Physics");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Physics not available: {}", e)),
        }
    }
    
    async fn ensure_world(&self) -> Result<World::Client> {
        let mut guard = self.world_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match World::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Player] Connected to World");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("World not available: {}", e)),
        }
    }
    
    async fn ensure_inventory(&self) -> Result<Inventory::Client> {
        let mut guard = self.inventory_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Inventory::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Player] Connected to Inventory");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Inventory not available: {}", e)),
        }
    }
    
    async fn ensure_factory(&self) -> Result<Factory::Client> {
        let mut guard = self.factory_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Factory::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Player] Connected to Factory");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Factory not available: {}", e)),
        }
    }
    
    async fn ensure_ui(&self) -> Result<UI::Client> {
        let mut guard = self.ui_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match UI::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Player] Connected to UI");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("UI not available: {}", e)),
        }
    }
    
    async fn ensure_asteroid(&self) -> Result<Asteroid::Client> {
        let mut guard = self.asteroid_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Asteroid::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Player] Connected to Asteroid");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Asteroid not available: {}", e)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("👤 Player Cell - Input & Camera Controller");
    println!("   └─ Players: 0");
    
    let service = PlayerService {
        players: Arc::new(RwLock::new(HashMap::new())),
        renderer_client: Arc::new(Mutex::new(None)),
        physics_client: Arc::new(Mutex::new(None)),
        world_client: Arc::new(Mutex::new(None)),
        inventory_client: Arc::new(Mutex::new(None)),
        factory_client: Arc::new(Mutex::new(None)),
        ui_client: Arc::new(Mutex::new(None)),
        asteroid_client: Arc::new(Mutex::new(None)),
    };
    
    // Auto-spawn player task
    let bootstrap_service = service.clone();
    tokio::spawn(async move {
        // Wait for other cells to wake up
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        
        tracing::info!("[Player] bootstrapping default player...");
        
        let req = SpawnPlayer {
            player_id: 1,
            name: "Commander Shepard".to_string(),
            spawn_position: [0.0, 10.0],
        };
        
        match bootstrap_service.spawn_player(req).await {
            Ok(_) => tracing::info!("[Player] ✅ Default player spawned!"),
            Err(e) => tracing::error!("[Player] Failed to spawn default player: {}", e),
        }
        
        // Set camera
        if let Ok(renderer) = bootstrap_service.ensure_renderer().await {
            let _ = renderer.set_camera(Renderer::SetCamera {
                camera_id: "main".to_string(),
                position: [0.0, 0.0, 50.0],
                target: [0.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0],
                fov: 45.0,
                near: 0.1,
                far: 1000.0,
            }).await;
        }
    });
    
    service.serve("player").await
}