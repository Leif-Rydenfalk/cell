//! Asteroid Cell - Procedural Generation & Mining
//!
//! This cell provides:
//! - Procedural asteroid field generation
//! - Resource distribution
//! - Mining mechanics
//! - Asteroid depletion and respawning

use anyhow::Result;
use cell_sdk::*;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

// ========= PROTEINS =========

#[protein]
pub struct AsteroidField {
    pub id: String,
    pub name: String,
    pub position: [f32; 2],
    pub radius: f32,
    pub asteroid_count: u32,
    pub resource_richness: f32,
    pub seed: u64,
    pub asteroids: Vec<Asteroid>,
    pub generated_at: u64,
}

#[protein]
pub struct Asteroid {
    pub id: u64,
    pub position: [f32; 2],
    pub size: f32,
    pub rotation_speed: f32,
    pub resources: Vec<(String, u32)>,
    pub depletion: f32, // 0.0 = full, 1.0 = empty
    pub entity_id: Option<u64>,
}

#[protein]
pub struct GenerateField {
    pub name: String,
    pub position: [f32; 2],
    pub radius: f32,
    pub count: u32,
    pub richness: f32,
    pub seed: Option<u64>,
}

#[protein]
pub struct GetField {
    pub field_id: String,
}

#[protein]
pub struct MineAsteroid {
    pub field_id: String,
    pub asteroid_id: u64,
    pub drill_power: u32,
    pub player_id: u64,
}

#[protein]
pub struct MineResult {
    pub resources: Vec<(String, u32)>,
    pub new_depletion: f32,
    pub depleted: bool,
}

#[protein]
pub struct ScanField {
    pub field_id: String,
    pub scanner_range: f32,
    pub position: [f32; 2],
}

#[protein]
pub struct ScanResult {
    pub asteroids: Vec<ScannedAsteroid>,
}

#[protein]
pub struct ScannedAsteroid {
    pub id: u64,
    pub position: [f32; 2],
    pub size: f32,
    pub estimated_value: u32,
    pub depletion: f32,
}

// Added local Ping struct for health checks
#[protein]
pub struct Ping;

// ========= SERVICE =========

#[service]
#[derive(Clone)]
struct AsteroidService {
    fields: Arc<RwLock<HashMap<String, AsteroidField>>>,
    next_asteroid_id: Arc<RwLock<u64>>,
    world_client: Arc<Mutex<Option<World::Client>>>,
    physics_client: Arc<Mutex<Option<Physics::Client>>>,
    inventory_client: Arc<Mutex<Option<Inventory::Client>>>,
}

cell_remote!(World = "world");
cell_remote!(Physics = "physics");
cell_remote!(Inventory = "inventory");

impl AsteroidService {
    fn generate_asteroid(
        &self,
        field_pos: [f32; 2],
        field_radius: f32,
        richness: f32,
        seed: u64,
        index: u32,
        next_id: u64,
    ) -> Asteroid {
        let mut rng = StdRng::seed_from_u64(seed ^ (index as u64 * 0x9e3779b97f4a7c15));
        
        // Random position within field
        let angle = rng.gen::<f32>() * std::f32::consts::TAU;
        let distance = rng.gen::<f32>().sqrt() * field_radius * 0.8;
        let x = field_pos[0] + angle.cos() * distance;
        let y = field_pos[1] + angle.sin() * distance;
        
        // Random size (log-normal distribution)
        let size = rng.gen::<f32>().powf(2.0) * 3.0 + 1.0;
        
        // Random rotation speed
        let rotation_speed = (rng.gen::<f32>() - 0.5) * 2.0;
        
        // Generate resources based on richness and size
        let mut resources = Vec::new();
        let resource_count = (size * richness * rng.gen::<f32>().powf(2.0) * 5.0) as u32;
        
        if resource_count > 0 {
            let ore_count = (resource_count as f32 * 0.7) as u32;
            if ore_count > 0 {
                resources.push(("ore_iron".to_string(), ore_count));
            }
            
            let copper_count = (resource_count as f32 * 0.3) as u32;
            if copper_count > 0 {
                resources.push(("ore_copper".to_string(), copper_count));
            }
            
            // Rare resources
            if rng.gen::<f32>() < 0.1 * richness {
                resources.push(("ore_titanium".to_string(), (size * 2.0) as u32));
            }
            if rng.gen::<f32>() < 0.05 * richness {
                resources.push(("crystal".to_string(), (size * 1.5) as u32));
            }
        }
        
        Asteroid {
            id: next_id,
            position: [x, y],
            size,
            rotation_speed,
            resources,
            depletion: 0.0,
            entity_id: None,
        }
    }
}

#[handler]
impl AsteroidService {
    async fn generate_field(&self, req: GenerateField) -> Result<AsteroidField> {
        let mut fields = self.fields.write().await;
        let mut next_id = self.next_asteroid_id.write().await;
        
        let field_id = format!("field_{}", fields.len() + 1);
        let seed = req.seed.unwrap_or_else(|| rand::random());
        
        let mut asteroids = Vec::new();
        
        for i in 0..req.count {
            let asteroid_id = *next_id;
            *next_id += 1;
            
            let asteroid = self.generate_asteroid(
                req.position,
                req.radius,
                req.richness,
                seed,
                i,
                asteroid_id,
            );
            
            asteroids.push(asteroid);
        }
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        let field = AsteroidField {
            id: field_id.clone(),
            name: req.name,
            position: req.position,
            radius: req.radius,
            asteroid_count: req.count,
            resource_richness: req.richness,
            seed,
            asteroids,
            generated_at: now,
        };
        
        fields.insert(field_id, field.clone());
        
        // Spawn asteroids in physics and world
        if let Ok(physics) = self.ensure_physics().await {
            if let Ok(world) = self.ensure_world().await {
                for asteroid in &field.asteroids {
                    let def = Physics::RigidBodyDef {
                        id: 0,
                        shape: Physics::Shape::Ball { radius: asteroid.size * 0.5 },
                        position: asteroid.position,
                        rotation: 0.0,
                        density: 1.0,
                        friction: 0.3,
                        restitution: 0.1,
                        is_static: true, // Asteroids don't move
                        collider_groups: Some([1, 1]),
                    };
                    
                        if let Ok(resp) = physics.spawn_body(Physics::SpawnBody { def }).await {
                        let _entity_id = resp.handle;
                        
                        // Spawn in world
                        let entity = World::Entity {
                            id: 0,
                            entity_type: World::EntityType::Asteroid,
                            position: asteroid.position,
                            rotation: 0.0,
                            scale: [asteroid.size, asteroid.size],
                            parent_id: None,
                            components: vec![
                                World::EntityComponent {
                                    component_type: "asteroid".to_string(),
                                    data: serde_json::to_string(&asteroid)?,
                                }
                            ],
                            metadata: vec![
                                ("field_id".to_string(), field.id.clone()),
                                ("asteroid_id".to_string(), asteroid.id.to_string()),
                                ("resources".to_string(), format!("{:?}", asteroid.resources)),
                            ],
                            created_at: now,
                            updated_at: now,
                        };
                        
                        let _ = world.spawn_entity(World::SpawnEntity { entity }).await;
                    }
                }
            }
        }
        
        Ok(field)
    }
    
    async fn get_field(&self, req: GetField) -> Result<Option<AsteroidField>> {
        let fields = self.fields.read().await;
        Ok(fields.get(&req.field_id).cloned())
    }
    
    async fn mine_asteroid(&self, req: MineAsteroid) -> Result<MineResult> {
        let mut fields = self.fields.write().await;
        
        if let Some(field) = fields.get_mut(&req.field_id) {
            if let Some(asteroid) = field.asteroids.iter_mut().find(|a| a.id == req.asteroid_id) {
                // Calculate yield based on drill power and asteroid size
                let yield_multiplier = (req.drill_power as f32 / 100.0).min(1.0);
                let remaining = 1.0 - asteroid.depletion;
                let yield_amount = (remaining * yield_multiplier * 0.1).min(remaining);
                
                asteroid.depletion += yield_amount;
                
                let mut harvested = Vec::new();
                for (resource, amount) in &asteroid.resources {
                    let harvest = (*amount as f32 * yield_amount) as u32;
                    if harvest > 0 {
                        harvested.push((resource.clone(), harvest));
                    }
                }
                
                // Add to player's inventory
                if let Ok(inventory) = self.ensure_inventory().await {
                    for (resource, amount) in &harvested {
                        let _ = inventory.deposit_item(Inventory::DepositItem {
                            player_id: req.player_id,
                            item_id: resource.clone(),
                            quantity: *amount as u64,
                            durability: None,
                            custom_data: None,
                        }).await;
                    }
                }
                
                let depleted = asteroid.depletion >= 0.99;
                
                if depleted {
                    // Remove asteroid from physics/world
                    if let Some(entity_id) = asteroid.entity_id {
                        if let Ok(world) = self.ensure_world().await {
                            let _ = world.despawn_entity(World::DespawnEntity { id: entity_id }).await;
                        }
                    }
                }
                
                return Ok(MineResult {
                    resources: harvested,
                    new_depletion: asteroid.depletion,
                    depleted,
                });
            }
        }
        
        Err(anyhow::anyhow!("Asteroid not found"))
    }
    
    async fn scan_field(&self, req: ScanField) -> Result<ScanResult> {
        let fields = self.fields.read().await;
        
        if let Some(field) = fields.get(&req.field_id) {
            let mut scanned = Vec::new();
            
            for asteroid in &field.asteroids {
                // Check distance
                let dx = asteroid.position[0] - req.position[0];
                let dy = asteroid.position[1] - req.position[1];
                let dist = (dx * dx + dy * dy).sqrt();
                
                if dist <= req.scanner_range {
                    // Estimate value based on remaining resources
                    let mut estimated_value = 0;
                    for (_, amount) in &asteroid.resources {
                        estimated_value += (amount * 10) as u32; // 10 credits per unit
                    }
                    estimated_value = (estimated_value as f32 * (1.0 - asteroid.depletion)) as u32;
                    
                    scanned.push(ScannedAsteroid {
                        id: asteroid.id,
                        position: asteroid.position,
                        size: asteroid.size,
                        estimated_value,
                        depletion: asteroid.depletion,
                    });
                }
            }
            
            Ok(ScanResult { asteroids: scanned })
        } else {
            Err(anyhow::anyhow!("Field not found"))
        }
    }
    
    async fn regenerate_field(&self, field_id: String) -> Result<()> {
        let mut fields = self.fields.write().await;
        
        if let Some(field) = fields.get_mut(&field_id) {
            // Remove old asteroids
            for asteroid in &field.asteroids {
                if let Some(entity_id) = asteroid.entity_id {
                    if let Ok(world) = self.ensure_world().await {
                        let _ = world.despawn_entity(World::DespawnEntity { id: entity_id }).await;
                    }
                }
            }
            
            // Generate new asteroids
            let mut next_id = self.next_asteroid_id.write().await;
            let mut new_asteroids = Vec::new();
            
            for i in 0..field.asteroid_count {
                let asteroid_id = *next_id;
                *next_id += 1;
                
                let asteroid = self.generate_asteroid(
                    field.position,
                    field.radius,
                    field.resource_richness,
                    field.seed + 1,
                    i,
                    asteroid_id,
                );
                
                new_asteroids.push(asteroid);
            }
            
            field.asteroids = new_asteroids;
            field.generated_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            
            // Spawn new asteroids
            if let Ok(physics) = self.ensure_physics().await {
                if let Ok(world) = self.ensure_world().await {
                    for asteroid in &field.asteroids {
                        let def = Physics::RigidBodyDef {
                            id: 0,
                            shape: Physics::Shape::Ball { radius: asteroid.size * 0.5 },
                            position: asteroid.position,
                            rotation: 0.0,
                            density: 1.0,
                            friction: 0.3,
                            restitution: 0.1,
                            is_static: true,
                            collider_groups: Some([1, 1]),
                        };
                        
                                if let Ok(resp) = physics.spawn_body(Physics::SpawnBody { def }).await {
                            let _entity_id = resp.handle;
                            
                            let entity = World::Entity {
                                id: 0,
                                entity_type: World::EntityType::Asteroid,
                                position: asteroid.position,
                                rotation: 0.0,
                                scale: [asteroid.size, asteroid.size],
                                parent_id: None,
                                components: vec![],
                                metadata: vec![
                                    ("field_id".to_string(), field.id.clone()),
                                    ("asteroid_id".to_string(), asteroid.id.to_string()),
                                ],
                                created_at: field.generated_at,
                                updated_at: field.generated_at,
                            };
                            
                            let _ = world.spawn_entity(World::SpawnEntity { entity }).await;
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
}

impl AsteroidService {
     async fn ensure_physics(&self) -> Result<Physics::Client> {
        let mut guard = self.physics_client.lock().await;
        
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Physics::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Asteroid] Connected to Physics");
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
                tracing::info!("[Asteroid] Connected to World");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Failed to connect to World: {}", e)),
        }
    }
    
       async fn ensure_inventory(&self) -> Result<Inventory::Client> {
        let mut guard = self.inventory_client.lock().await;
        
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        
        match Inventory::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Asteroid] Connected to Inventory");
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Failed to connect to Inventory: {}", e)),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🪐 Asteroid Cell - Procedural Generation & Mining");
    println!("   └─ Fields: 0");
    println!("   └─ Asteroids: 0");
    
    let service = AsteroidService {
        fields: Arc::new(RwLock::new(HashMap::new())),
        next_asteroid_id: Arc::new(RwLock::new(1)),
        world_client: Arc::new(Mutex::new(None)),
        physics_client: Arc::new(Mutex::new(None)),
        inventory_client: Arc::new(Mutex::new(None)),
    };
    
    service.serve("asteroid").await
}