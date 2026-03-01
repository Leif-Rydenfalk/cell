//! Factory Cell - Ship Building & Production Lines
//!
//! This cell provides:
//! - Ship blueprint management
//! - Component placement on grid
//! - Production queue for crafting
//! - Ship assembly from components

use anyhow::Result;
use cell_sdk::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use std::time::Duration;

// ========= PROTEINS (PUBLIC API) =========

#[protein]
pub struct ShipBlueprint {
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner_id: u64,
    pub width: u32,
    pub height: u32,
    pub components: Vec<PlacedComponent>,
    pub stats: ShipStats,
    pub created_at: u64,
    pub version: u32,
}

#[protein]
pub struct PlacedComponent {
    pub component_id: String,
    pub grid_x: i32,
    pub grid_y: i32,
    pub rotation: i32,
    pub flipped: bool,
}

#[protein]
pub struct ComponentDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: ComponentCategory,
    pub size: [u32; 2],
    pub mass: f32,
    pub hp: u32,
    pub power_consumption: i32,
    pub thrust: Option<[f32; 2]>,
    pub cargo_capacity: Option<u32>,
    pub drill_power: Option<u32>,
    pub scanner_range: Option<f32>,
    pub shield_capacity: Option<u32>,
    pub reactor_output: Option<u32>,
    pub recipe_id: Option<String>,
    pub required_level: u32,
    // Changed to Vec for reliable rkyv serialization
    pub cost: Vec<(String, u32)>,
}

#[protein]
pub enum ComponentCategory {
    Hull,
    Thruster,
    Reactor,
    Cargo,
    Drill,
    Scanner,
    Shield,
    Weapon,
    Utility,
    Decoration,
}

#[protein]
pub struct ShipStats {
    pub total_mass: f32,
    pub total_thrust: f32,
    pub acceleration: f32,
    pub rotation_speed: f32,
    pub cargo_capacity: u32,
    pub power_generation: i32,
    pub power_consumption: i32,
    pub shield_capacity: u32,
    pub hull_integrity: u32,
    pub drill_power: u32,
    pub scanner_range: f32,
}

#[protein]
pub struct CreateBlueprint {
    pub name: String,
    pub description: String,
    pub owner_id: u64,
    pub width: u32,
    pub height: u32,
}

#[protein]
pub struct PlaceComponent {
    pub blueprint_id: String,
    pub component_id: String,
    pub grid_x: i32,
    pub grid_y: i32,
    pub rotation: i32,
    pub flipped: bool,
}

#[protein]
pub struct RemoveComponent {
    pub blueprint_id: String,
    pub grid_x: i32,
    pub grid_y: i32,
}

#[protein]
pub struct GetBlueprint {
    pub blueprint_id: String,
}

#[protein]
pub struct ListBlueprints {
    pub owner_id: Option<u64>,
}

#[protein]
pub struct SaveBlueprint {
    pub blueprint: ShipBlueprint,
}

#[protein]
pub struct DeleteBlueprint {
    pub blueprint_id: String,
}

#[protein]
pub struct AssembleShip {
    pub blueprint_id: String,
    pub player_id: u64,
    pub spawn_position: [f32; 2],
}

#[protein]
pub struct AssemblyResult {
    pub ship_entity_id: u64,
    pub components_consumed: Vec<(String, u32)>,
    pub total_cost: u32,
}

#[protein]
pub struct ProductionJob {
    pub id: String,
    pub recipe_id: String,
    pub player_id: u64,
    pub quantity: u32,
    pub progress: f32,
    pub status: JobStatus,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}

#[protein]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[protein]
pub struct QueueProduction {
    pub player_id: u64,
    pub recipe_id: String,
    pub quantity: u32,
}

#[protein]
pub struct CancelProduction {
    pub job_id: String,
}

#[protein]
pub struct GetProductionQueue {
    pub player_id: u64,
}

#[protein]
pub struct GetComponentDefinitions {
    pub category: Option<ComponentCategory>,
}

#[protein]
pub struct Ping;

// ========= SERVICE =========

#[service]
#[derive(Clone)]
struct FactoryService {
    blueprints: Arc<RwLock<HashMap<String, ShipBlueprint>>>,
    components: Arc<RwLock<HashMap<String, ComponentDefinition>>>,
    production_queue: Arc<RwLock<HashMap<String, ProductionJob>>>,
    next_blueprint_id: Arc<RwLock<u64>>,
    next_job_id: Arc<RwLock<u64>>,
    inventory_client: Arc<Mutex<Option<Inventory::Client>>>,
    physics_client: Arc<Mutex<Option<Physics::Client>>>,
    world_client: Arc<Mutex<Option<World::Client>>>,
}

cell_remote!(Inventory = "inventory");
cell_remote!(Physics = "physics");
cell_remote!(World = "world");

impl FactoryService {
    async fn init_default_components(&self) {
        let mut components = self.components.write().await;
        
        components.insert("hull_basic".to_string(), ComponentDefinition {
            id: "hull_basic".to_string(),
            name: "Basic Hull".to_string(),
            description: "Standard structural component".to_string(),
            category: ComponentCategory::Hull,
            size: [1, 1],
            mass: 1.0,
            hp: 100,
            power_consumption: 0,
            thrust: None,
            cargo_capacity: None,
            drill_power: None,
            scanner_range: None,
            shield_capacity: None,
            reactor_output: None,
            recipe_id: Some("craft_hull_basic".to_string()),
            required_level: 1,
            cost: vec![("plate_iron".to_string(), 2)],
        });
        
        components.insert("thruster_small".to_string(), ComponentDefinition {
            id: "thruster_small".to_string(),
            name: "Small Thruster".to_string(),
            description: "Provides 100N of thrust".to_string(),
            category: ComponentCategory::Thruster,
            size: [1, 1],
            mass: 0.5,
            hp: 50,
            power_consumption: 10,
            thrust: Some([0.0, 100.0]),
            cargo_capacity: None,
            drill_power: None,
            scanner_range: None,
            shield_capacity: None,
            reactor_output: None,
            recipe_id: Some("craft_thruster_small".to_string()),
            required_level: 1,
            cost: vec![("plate_iron".to_string(), 5)],
        });
        
        components.insert("cargo_small".to_string(), ComponentDefinition {
            id: "cargo_small".to_string(),
            name: "Small Cargo Bay".to_string(),
            description: "Holds 100 units of ore".to_string(),
            category: ComponentCategory::Cargo,
            size: [1, 1],
            mass: 2.0,
            hp: 80,
            power_consumption: 1,
            thrust: None,
            cargo_capacity: Some(100),
            drill_power: None,
            scanner_range: None,
            shield_capacity: None,
            reactor_output: None,
            recipe_id: Some("craft_cargo_small".to_string()),
            required_level: 1,
            cost: vec![("plate_iron".to_string(), 10)],
        });
        
        components.insert("drill_basic".to_string(), ComponentDefinition {
            id: "drill_basic".to_string(),
            name: "Basic Mining Drill".to_string(),
            description: "Extracts ore from asteroids".to_string(),
            category: ComponentCategory::Drill,
            size: [1, 1],
            mass: 3.0,
            hp: 120,
            power_consumption: 20,
            thrust: None,
            cargo_capacity: None,
            drill_power: Some(50),
            scanner_range: None,
            shield_capacity: None,
            reactor_output: None,
            recipe_id: Some("craft_drill_basic".to_string()),
            required_level: 2,
            cost: vec![("plate_iron".to_string(), 20)],
        });
        
        components.insert("reactor_small".to_string(), ComponentDefinition {
            id: "reactor_small".to_string(),
            name: "Small Fusion Reactor".to_string(),
            description: "Generates 50 power units".to_string(),
            category: ComponentCategory::Reactor,
            size: [2, 2],
            mass: 10.0,
            hp: 200,
            power_consumption: -50,
            thrust: None,
            cargo_capacity: None,
            drill_power: None,
            scanner_range: None,
            shield_capacity: None,
            reactor_output: Some(50),
            recipe_id: Some("craft_reactor_small".to_string()),
            required_level: 3,
            cost: vec![("plate_iron".to_string(), 50)],
        });
        
        components.insert("shield_basic".to_string(), ComponentDefinition {
            id: "shield_basic".to_string(),
            name: "Basic Shield Generator".to_string(),
            description: "Absorbs 500 damage".to_string(),
            category: ComponentCategory::Shield,
            size: [1, 1],
            mass: 2.0,
            hp: 100,
            power_consumption: 30,
            thrust: None,
            cargo_capacity: None,
            drill_power: None,
            scanner_range: None,
            shield_capacity: Some(500),
            reactor_output: None,
            recipe_id: Some("craft_shield_basic".to_string()),
            required_level: 2,
            cost: vec![("plate_iron".to_string(), 30)],
        });
    }
    
    fn calculate_ship_stats(&self, blueprint: &ShipBlueprint) -> ShipStats {
        let mut total_mass = 0.0;
        let mut total_thrust = 0.0;
        let mut cargo_capacity = 0;
        let mut power_generation = 0;
        let mut power_consumption = 0;
        let mut shield_capacity = 0;
        let mut hull_integrity = 0;
        let mut drill_power = 0;
        let mut scanner_range: f32 = 0.0; // Fixed type inference
        
        let components = std::sync::Arc::clone(&self.components);
        let components_guard = components.blocking_read();
        
        for placed in &blueprint.components {
            if let Some(def) = components_guard.get(&placed.component_id) {
                total_mass += def.mass;
                hull_integrity += def.hp;
                
                if def.power_consumption < 0 {
                    power_generation += -def.power_consumption;
                } else {
                    power_consumption += def.power_consumption;
                }
                
                if let Some(thrust) = def.thrust {
                    total_thrust += thrust[1];
                }
                
                if let Some(capacity) = def.cargo_capacity {
                    cargo_capacity += capacity;
                }
                
                if let Some(capacity) = def.shield_capacity {
                    shield_capacity += capacity;
                }
                
                if let Some(power) = def.drill_power {
                    drill_power += power;
                }
                
                if let Some(range) = def.scanner_range {
                    scanner_range = scanner_range.max(range);
                }
            }
        }
        
        let acceleration = if total_mass > 0.0 {
            total_thrust / total_mass
        } else {
            0.0
        };
        
        ShipStats {
            total_mass,
            total_thrust,
            acceleration,
            rotation_speed: 3.0,
            cargo_capacity,
            power_generation,
            power_consumption,
            shield_capacity,
            hull_integrity,
            drill_power,
            scanner_range,
        }
    }
    
    fn check_component_placement(
        &self,
        blueprint: &ShipBlueprint,
        component: &ComponentDefinition,
        x: i32,
        y: i32,
        rotation: i32,
    ) -> bool {
        // Fixed destructuring of array
        let [width, height] = component.size;
        
        let (w, h) = if rotation % 2 == 1 {
            (height as i32, width as i32)
        } else {
            (width as i32, height as i32)
        };
        
        if x < 0 || y < 0 || x + w > blueprint.width as i32 || y + h > blueprint.height as i32 {
            return false;
        }
        
        let components = std::sync::Arc::clone(&self.components);
        let def = components.blocking_read();
        
        for placed in &blueprint.components {
            if let Some(def2) = def.get(&placed.component_id) {
                let (pw, ph) = if placed.rotation % 2 == 1 {
                    (def2.size[1] as i32, def2.size[0] as i32)
                } else {
                    (def2.size[0] as i32, def2.size[1] as i32)
                };
                
                let rect1 = (x, y, w, h);
                let rect2 = (placed.grid_x, placed.grid_y, pw, ph);
                
                if rects_overlap(rect1, rect2) {
                    return false;
                }
            }
        }
        
        true
    }
}

fn rects_overlap(r1: (i32, i32, i32, i32), r2: (i32, i32, i32, i32)) -> bool {
    r1.0 < r2.0 + r2.2 &&
    r1.0 + r1.2 > r2.0 &&
    r1.1 < r2.1 + r2.3 &&
    r1.1 + r1.3 > r2.1
}

#[handler]
impl FactoryService {
    async fn ping(&self, _req: Ping) -> Result<()> {
        Ok(())
    }
    
    async fn get_component_definitions(&self, req: GetComponentDefinitions) -> Result<Vec<ComponentDefinition>> {
        let components = self.components.read().await;
        
        let result: Vec<ComponentDefinition> = components
            .values()
            .filter(|c| {
                if let Some(cat) = &req.category {
                    &c.category == cat
                } else {
                    true
                }
            })
            .cloned()
            .collect();
        
        Ok(result)
    }
    
    async fn create_blueprint(&self, req: CreateBlueprint) -> Result<ShipBlueprint> {
        let mut blueprints = self.blueprints.write().await;
        let mut next_id = self.next_blueprint_id.write().await;
        
        let id = format!("bp_{}", *next_id);
        *next_id += 1;
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        let blueprint = ShipBlueprint {
            id: id.clone(),
            name: req.name,
            description: req.description,
            owner_id: req.owner_id,
            width: req.width,
            height: req.height,
            components: Vec::new(),
            stats: ShipStats {
                total_mass: 0.0,
                total_thrust: 0.0,
                acceleration: 0.0,
                rotation_speed: 0.0,
                cargo_capacity: 0,
                power_generation: 0,
                power_consumption: 0,
                shield_capacity: 0,
                hull_integrity: 0,
                drill_power: 0,
                scanner_range: 0.0,
            },
            created_at: now,
            version: 1,
        };
        
        blueprints.insert(id, blueprint.clone());
        
        Ok(blueprint)
    }
    
    async fn place_component(&self, req: PlaceComponent) -> Result<ShipBlueprint> {
        let mut blueprints = self.blueprints.write().await;
        
        if let Some(blueprint) = blueprints.get_mut(&req.blueprint_id) {
            let components = self.components.read().await;
            
            if let Some(component) = components.get(&req.component_id) {
                if !self.check_component_placement(
                    blueprint,
                    component,
                    req.grid_x,
                    req.grid_y,
                    req.rotation,
                ) {
                    return Err(anyhow::anyhow!("Invalid component placement"));
                }
                
                blueprint.components.retain(|c| {
                    c.grid_x != req.grid_x || c.grid_y != req.grid_y
                });
                
                blueprint.components.push(PlacedComponent {
                    component_id: req.component_id,
                    grid_x: req.grid_x,
                    grid_y: req.grid_y,
                    rotation: req.rotation,
                    flipped: req.flipped,
                });
                
                blueprint.version += 1;
                blueprint.stats = self.calculate_ship_stats(blueprint);
                
                Ok(blueprint.clone())
            } else {
                Err(anyhow::anyhow!("Component not found: {}", req.component_id))
            }
        } else {
            Err(anyhow::anyhow!("Blueprint not found: {}", req.blueprint_id))
        }
    }
    
    async fn remove_component(&self, req: RemoveComponent) -> Result<ShipBlueprint> {
        let mut blueprints = self.blueprints.write().await;
        
        if let Some(blueprint) = blueprints.get_mut(&req.blueprint_id) {
            blueprint.components.retain(|c| {
                c.grid_x != req.grid_x || c.grid_y != req.grid_y
            });
            
            blueprint.version += 1;
            blueprint.stats = self.calculate_ship_stats(blueprint);
            
            Ok(blueprint.clone())
        } else {
            Err(anyhow::anyhow!("Blueprint not found: {}", req.blueprint_id))
        }
    }
    
    async fn get_blueprint(&self, req: GetBlueprint) -> Result<Option<ShipBlueprint>> {
        let blueprints = self.blueprints.read().await;
        Ok(blueprints.get(&req.blueprint_id).cloned())
    }
    
    async fn list_blueprints(&self, req: ListBlueprints) -> Result<Vec<ShipBlueprint>> {
        let blueprints = self.blueprints.read().await;
        
        let result: Vec<ShipBlueprint> = blueprints
            .values()
            .filter(|bp| {
                if let Some(owner_id) = req.owner_id {
                    bp.owner_id == owner_id
                } else {
                    true
                }
            })
            .cloned()
            .collect();
        
        Ok(result)
    }
    
    async fn save_blueprint(&self, req: SaveBlueprint) -> Result<()> {
        let mut blueprints = self.blueprints.write().await;
        blueprints.insert(req.blueprint.id.clone(), req.blueprint);
        Ok(())
    }
    
    async fn delete_blueprint(&self, req: DeleteBlueprint) -> Result<()> {
        let mut blueprints = self.blueprints.write().await;
        blueprints.remove(&req.blueprint_id);
        Ok(())
    }
    
    async fn assemble_ship(&self, req: AssembleShip) -> Result<AssemblyResult> {
        let blueprint = self.get_blueprint(GetBlueprint { blueprint_id: req.blueprint_id }).await?
            .ok_or_else(|| anyhow::anyhow!("Blueprint not found"))?;
        
        if blueprint.owner_id != req.player_id {
            return Err(anyhow::anyhow!("You don't own this blueprint"));
        }
        
        let components = self.components.read().await;
        let mut required_items: HashMap<String, u32> = HashMap::new();
        
        for placed in &blueprint.components {
            if let Some(def) = components.get(&placed.component_id) {
                for (item_id, quantity) in &def.cost {
                    *required_items.entry(item_id.clone()).or_insert(0) += quantity;
                }
            }
        }
        
        drop(components);
        
        let inventory = self.ensure_inventory().await?;
        
        for (item_id, quantity) in &required_items {
            let inv = inventory.get_inventory(Inventory::GetInventory {
                player_id: req.player_id,
            }).await?;
            
            let available: u64 = inv.items
                .iter()
                .filter(|i| &i.item_id == item_id)
                .map(|i| i.quantity)
                .sum();
            
            if available < *quantity as u64 {
                return Err(anyhow::anyhow!(
                    "Insufficient {}: need {}, have {}",
                    item_id, quantity, available
                ));
            }
        }
        
        for (item_id, quantity) in &required_items {
            inventory.withdraw_item(Inventory::WithdrawItem {
                player_id: req.player_id,
                item_id: item_id.clone(),
                quantity: *quantity as u64,
            }).await?;
        }
        
        let physics = self.ensure_physics().await?;
        
        let components_lock = self.components.read().await;
        let mut center_x = 0.0;
        let mut center_y = 0.0;
        let mut total_mass = 0.0;
        
        for placed in &blueprint.components {
            if let Some(def) = components_lock.get(&placed.component_id) {
                let world_x = req.spawn_position[0] + placed.grid_x as f32 * 0.5;
                let world_y = req.spawn_position[1] + placed.grid_y as f32 * 0.5;
                
                center_x += world_x * def.mass;
                center_y += world_y * def.mass;
                total_mass += def.mass;
            }
        }
        
        if total_mass > 0.0 {
            center_x /= total_mass;
            center_y /= total_mass;
        }
        
        drop(components_lock);
        
        let ship_entity = physics.spawn_body(Physics::SpawnBody {
            def: Physics::RigidBodyDef {
                id: 0,
                shape: Physics::Shape::Cuboid { 
                    hx: blueprint.width as f32 * 0.25, 
                    hy: blueprint.height as f32 * 0.25 
                },
                position: [center_x, center_y],
                rotation: 0.0,
                density: total_mass / (blueprint.width as f32 * blueprint.height as f32 * 0.25),
                friction: 0.5,
                restitution: 0.2,
                is_static: false,
                collider_groups: None,
            },
        }).await?.handle;
        
        if let Ok(world) = self.ensure_world().await {
            let _ = world.on_ship_assembled(World::OnShipAssembled {
                ship_id: ship_entity,
                blueprint_id: blueprint.id,
                owner_id: req.player_id,
                position: [center_x, center_y],
            }).await;
        }
        
        // Calculate cost first to avoid move errors
        let total_cost = required_items.values().sum::<u32>();
        
        Ok(AssemblyResult {
            ship_entity_id: ship_entity,
            components_consumed: required_items.into_iter().collect(),
            total_cost,
        })
    }
    
    async fn queue_production(&self, req: QueueProduction) -> Result<ProductionJob> {
        let mut queue = self.production_queue.write().await;
        let mut next_id = self.next_job_id.write().await;
        
        let id = format!("job_{}", *next_id);
        *next_id += 1;
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        let job = ProductionJob {
            id: id.clone(),
            recipe_id: req.recipe_id,
            player_id: req.player_id,
            quantity: req.quantity,
            progress: 0.0,
            status: JobStatus::Queued,
            started_at: now,
            completed_at: None,
        };
        
        queue.insert(id, job.clone());
        
        Ok(job)
    }
    
    async fn cancel_production(&self, req: CancelProduction) -> Result<()> {
        let mut queue = self.production_queue.write().await;
        
        if let Some(job) = queue.get_mut(&req.job_id) {
            job.status = JobStatus::Cancelled;
        }
        
        Ok(())
    }
    
    async fn get_production_queue(&self, req: GetProductionQueue) -> Result<Vec<ProductionJob>> {
        let queue = self.production_queue.read().await;
        
        let result: Vec<ProductionJob> = queue
            .values()
            .filter(|j| j.player_id == req.player_id)
            .cloned()
            .collect();
        
        Ok(result)
    }
}

impl FactoryService {
    async fn ensure_inventory(&self) -> Result<Inventory::Client> {
        let mut guard = self.inventory_client.lock().await;
        
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Inventory::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Factory] Connected to Inventory");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Failed to connect to Inventory: {}", e)),
        }
    }
    
    async fn ensure_physics(&self) -> Result<Physics::Client> {
        let mut guard = self.physics_client.lock().await;
        
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Physics::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Factory] Connected to Physics");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Failed to connect to Physics: {}", e)),
        }
    }
    
    async fn ensure_world(&self) -> Result<World::Client> {
        let mut guard = self.world_client.lock().await;
        
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match World::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Factory] Connected to World");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Failed to connect to World: {}", e)),
        }
    }
    
    async fn production_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        
        loop {
            interval.tick().await;
            
            let mut queue = self.production_queue.write().await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            for job in queue.values_mut() {
                match job.status {
                    JobStatus::Queued => {
                        job.status = JobStatus::Running;
                    }
                    JobStatus::Running => {
                        job.progress += 1.0 / 60.0;
                        
                        if job.progress >= 1.0 {
                            job.status = JobStatus::Completed;
                            job.completed_at = Some(now);
                            job.progress = 1.0;
                        }
                    }
                    _ => {}
                }
            }
            
            queue.retain(|_, job| {
                if let Some(completed) = job.completed_at {
                    now - completed < 3600
                } else {
                    true
                }
            });
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🏭 Factory Cell - Ship Building & Production");
    println!("   └─ Blueprints: 0");
    println!("   └─ Components: 6");
    println!("   └─ Production Queue: 0");
    
    let service = FactoryService {
        blueprints: Arc::new(RwLock::new(HashMap::new())),
        components: Arc::new(RwLock::new(HashMap::new())),
        production_queue: Arc::new(RwLock::new(HashMap::new())),
        next_blueprint_id: Arc::new(RwLock::new(1)),
        next_job_id: Arc::new(RwLock::new(1)),
        inventory_client: Arc::new(Mutex::new(None)),
        physics_client: Arc::new(Mutex::new(None)),
        world_client: Arc::new(Mutex::new(None)),
    };
    
    service.init_default_components().await;
    
    let prod_service = service.clone();
    tokio::spawn(async move {
        prod_service.production_loop().await;
    });
    
    service.serve("factory").await
}