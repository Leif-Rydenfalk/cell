//! World Cell - Authoritative State Manager
//!
//! This cell is the SOURCE OF TRUTH for all game state.
//! It:
//! - Maintains entity list with SQLite persistence
//! - Receives physics snapshots and updates transforms
//! - Manages renderer connections and respawns entities
//! - Handles player sessions
//! - Persists ship blueprints, asteroid fields, etc.

use anyhow::Result;
use cell_sdk::*;
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

// ========= PROTEINS =========

#[protein]
pub struct Entity {
    pub id: u64,
    pub entity_type: EntityType,
    pub position: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
    pub parent_id: Option<u64>,
    pub components: Vec<EntityComponent>,
    // Changed to Vec for reliable rkyv serialization without extra trait bounds
    pub metadata: Vec<(String, String)>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[protein]
pub enum EntityType {
    Ship,
    Asteroid,
    Station,
    Player,
    Drone,
    Debris,
    Projectile,
    Effect,
}

#[protein]
pub struct EntityComponent {
    pub component_type: String,
    pub data: String, // JSON blob
}

#[protein]
pub struct PhysicsSnapshot {
    pub timestamp: u64,
    pub bodies: Vec<PhysicsBodySnapshot>,
}

#[protein]
pub struct PhysicsBodySnapshot {
    pub id: u64,
    pub position: [f32; 2],
    pub rotation: f32,
    pub linvel: [f32; 2],
    pub angvel: f32,
}

#[protein]
pub struct OnPhysicsTick {
    pub snapshot: PhysicsSnapshot,
}

#[protein]
pub struct OnBodySpawned {
    pub body_id: u64,
}

#[protein]
pub struct OnBodyDespawned {
    pub body_id: u64,
}

#[protein]
pub struct OnShipAssembled {
    pub ship_id: u64,
    pub blueprint_id: String,
    pub owner_id: u64,
    pub position: [f32; 2],
}

#[protein]
pub struct SpawnEntity {
    pub entity: Entity,
}

#[protein]
pub struct DespawnEntity {
    pub id: u64,
}

#[protein]
pub struct UpdateTransform {
    pub id: u64,
    pub position: [f32; 2],
    pub rotation: f32,
}

#[protein]
pub struct BatchUpdateTransforms {
    pub updates: Vec<UpdateTransform>,
}

#[protein]
pub struct GetEntity {
    pub id: u64,
}

#[protein]
pub struct QueryEntities {
    pub entity_type: Option<EntityType>,
    pub aabb_min: Option<[f32; 2]>,
    pub aabb_max: Option<[f32; 2]>,
    pub owner_id: Option<u64>,
}

#[protein]
pub struct QueryResult {
    pub entities: Vec<Entity>,
}

#[protein]
pub struct RegisterRenderer;
#[protein]
pub struct UnregisterRenderer;

#[protein]
pub struct PlayerSession {
    pub player_id: u64,
    pub name: String,
    pub ship_id: Option<u64>,
    pub camera_position: [f32; 2],
    pub connected_at: u64,
    pub last_seen: u64,
}

#[protein]
pub struct JoinGame {
    pub player_id: u64,
    pub name: String,
}

#[protein]
pub struct LeaveGame {
    pub player_id: u64,
}

// Added Ping/Pong proteins for health checks
#[protein]
pub struct Ping;
#[protein]
pub struct Pong;

// ========= SERVICE =========

#[service]
#[derive(Clone)]
struct WorldService {
    db: Arc<Mutex<Connection>>,
    db_path: PathBuf,
    entities: Arc<RwLock<HashMap<u64, Entity>>>,
    renderer_client: Arc<Mutex<Option<Renderer::Client>>>,
    next_entity_id: Arc<RwLock<u64>>,
    player_sessions: Arc<RwLock<HashMap<u64, PlayerSession>>>,
}

cell_remote!(Renderer = "renderer");

impl WorldService {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
        ")?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS entities (
                id INTEGER PRIMARY KEY,
                entity_type INTEGER NOT NULL,
                position_x REAL NOT NULL,
                position_y REAL NOT NULL,
                rotation REAL NOT NULL,
                scale_x REAL NOT NULL,
                scale_y REAL NOT NULL,
                parent_id INTEGER,
                metadata TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS entity_components (
                entity_id INTEGER NOT NULL,
                component_type TEXT NOT NULL,
                data TEXT NOT NULL,
                FOREIGN KEY (entity_id) REFERENCES entities(id) ON DELETE CASCADE,
                PRIMARY KEY (entity_id, component_type)
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS player_sessions (
                player_id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                ship_id INTEGER,
                camera_x REAL NOT NULL,
                camera_y REAL NOT NULL,
                connected_at INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                FOREIGN KEY (ship_id) REFERENCES entities(id)
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS blueprints (
                id TEXT PRIMARY KEY,
                owner_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                data TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        Ok(())
    }
    
    async fn respawn_all_entities(&self, renderer: &Renderer::Client) -> Result<()> {
        let entities = self.entities.read().await;
        let mut count = 0;
        
        for entity in entities.values() {
            let pass_id = match entity.entity_type {
                EntityType::Ship => "ship".to_string(),
                EntityType::Asteroid => "asteroid".to_string(),
                EntityType::Station => "station".to_string(),
                EntityType::Player => "player".to_string(),
                EntityType::Drone => "drone".to_string(),
                EntityType::Debris => "debris".to_string(),
                EntityType::Projectile => "projectile".to_string(),
                EntityType::Effect => "effect".to_string(),
            };
            
            let transform = [
                entity.scale[0], 0.0, 0.0, 0.0,
                0.0, entity.scale[1], 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                entity.position[0], entity.position[1], 0.0, 1.0,
            ];
            
            if let Err(e) = renderer.spawn_entity(Renderer::SpawnEntity {
                entity_id: format!("entity_{}", entity.id),
                pass_id,
                buffer_id: "cube".to_string(),
                vertex_count: 36,
                instance_count: 1,
                transform,
            }).await {
                tracing::warn!("Failed to respawn entity {}: {}", entity.id, e);
            } else {
                count += 1;
            }
        }
        
        tracing::info!("Respawned {} entities in new Renderer", count);
        Ok(())
    }
    
    async fn ensure_renderer(&self) -> Result<Renderer::Client> {
        let mut guard = self.renderer_client.lock().await;
        
        if let Some(client) = guard.as_ref() {
            if client.ping(Renderer::Ping).await.is_ok() {
                return Ok(client.clone());
            }
        }
        
        match Renderer::Client::connect().await {
            Ok(client) => {
                tracing::info!("[World] Connected to Renderer");
                
                // Respawn all entities
                if let Err(e) = self.respawn_all_entities(&client).await {
                    tracing::error!("Failed to respawn entities: {}", e);
                }
                
                *guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => Err(anyhow::anyhow!("Renderer not available: {}", e)),
        }
    }
}

#[handler]
impl WorldService {
    async fn ping(&self, _req: Ping) -> Result<Pong> {
        Ok(Pong)
    }
    
    async fn on_physics_tick(&self, req: OnPhysicsTick) -> Result<()> {
    let mut entities = self.entities.write().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    
    for body in &req.snapshot.bodies {
        if let Some(entity) = entities.get_mut(&body.id) {
            entity.position = body.position;
            entity.rotation = body.rotation;
            entity.updated_at = now;
        }
    }
    
    // Forward transforms to renderer if connected
    if let Ok(renderer) = self.ensure_renderer().await {
        let updates: Vec<Renderer::TransformUpdate> = req.snapshot.bodies
            .iter()
            .map(|body| Renderer::TransformUpdate {
                    entity_id: format!("entity_{}", body.id),
                    transform: [
                        1.0, 0.0, 0.0, 0.0,
                        0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0, 0.0,
                        body.position[0], body.position[1], 0.0, 1.0,
                    ],
                })
                .collect();
            
            let _ = renderer.batch_update_transforms(Renderer::BatchUpdateTransforms {
                updates,
            }).await;
        }
        
        Ok(())
    }
    
    async fn on_body_spawned(&self, req: OnBodySpawned) -> Result<()> {
        let mut entities = self.entities.write().await;
        
        if !entities.contains_key(&req.body_id) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            
            entities.insert(req.body_id, Entity {
                id: req.body_id,
                entity_type: EntityType::Ship,
                position: [0.0, 0.0],
                rotation: 0.0,
                scale: [1.0, 1.0],
                parent_id: None,
                components: Vec::new(),
                metadata: Vec::new(),
                created_at: now,
                updated_at: now,
            });
        }
        
        Ok(())
    }
    
    async fn on_body_despawned(&self, req: OnBodyDespawned) -> Result<()> {
        let mut entities = self.entities.write().await;
        entities.remove(&req.body_id);
        
        // Notify renderer
        if let Ok(renderer) = self.ensure_renderer().await {
            let _ = renderer.despawn_entity(Renderer::DespawnEntity {
                entity_id: format!("entity_{}", req.body_id),
            }).await;
        }
        
        Ok(())
    }
    
    async fn on_ship_assembled(&self, req: OnShipAssembled) -> Result<()> {
        let mut entities = self.entities.write().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        entities.insert(req.ship_id, Entity {
            id: req.ship_id,
            entity_type: EntityType::Ship,
            position: req.position,
            rotation: 0.0,
            scale: [1.0, 1.0],
            parent_id: None,
            components: Vec::new(),
            metadata: vec![
                ("blueprint_id".to_string(), req.blueprint_id),
                ("owner_id".to_string(), req.owner_id.to_string()),
            ],
            created_at: now,
            updated_at: now,
        });
        
        // Update player session
        let mut sessions = self.player_sessions.write().await;
        if let Some(session) = sessions.get_mut(&req.owner_id) {
            session.ship_id = Some(req.ship_id);
        }
        
        Ok(())
    }
    
    async fn spawn_entity(&self, req: SpawnEntity) -> Result<u64> {
        let mut entities = self.entities.write().await;
        let mut next_id = self.next_entity_id.write().await;
        
        let id = *next_id;
        *next_id += 1;
        
        let mut entity = req.entity;
        entity.id = id;
        
        entities.insert(id, entity.clone());
        
        // Spawn in renderer
        if let Ok(renderer) = self.ensure_renderer().await {
            let transform = [
                entity.scale[0], 0.0, 0.0, 0.0,
                0.0, entity.scale[1], 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                entity.position[0], entity.position[1], 0.0, 1.0,
            ];
            
            let pass_id = match entity.entity_type {
                EntityType::Ship => "ship".to_string(),
                EntityType::Asteroid => "asteroid".to_string(),
                EntityType::Station => "station".to_string(),
                EntityType::Player => "player".to_string(),
                EntityType::Drone => "drone".to_string(),
                EntityType::Debris => "debris".to_string(),
                EntityType::Projectile => "projectile".to_string(),
                EntityType::Effect => "effect".to_string(),
            };
            
            let _ = renderer.spawn_entity(Renderer::SpawnEntity {
                entity_id: format!("entity_{}", id),
                pass_id,
                buffer_id: "cube".to_string(),
                vertex_count: 36,
                instance_count: 1,
                transform,
            }).await;
        }
        
        Ok(id)
    }
    
    async fn despawn_entity(&self, req: DespawnEntity) -> Result<()> {
        let mut entities = self.entities.write().await;
        entities.remove(&req.id);
        
        // Despawn in renderer
        if let Ok(renderer) = self.ensure_renderer().await {
            let _ = renderer.despawn_entity(Renderer::DespawnEntity {
                entity_id: format!("entity_{}", req.id),
            }).await;
        }
        
        Ok(())
    }
    
    async fn update_transform(&self, req: UpdateTransform) -> Result<()> {
        let mut entities = self.entities.write().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        if let Some(entity) = entities.get_mut(&req.id) {
            entity.position = req.position;
            entity.rotation = req.rotation;
            entity.updated_at = now;
        }
        
        Ok(())
    }
    
    async fn batch_update_transforms(&self, req: BatchUpdateTransforms) -> Result<()> {
        let mut entities = self.entities.write().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        for update in req.updates {
            if let Some(entity) = entities.get_mut(&update.id) {
                entity.position = update.position;
                entity.rotation = update.rotation;
                entity.updated_at = now;
            }
        }
        
        Ok(())
    }
    
    async fn get_entity(&self, req: GetEntity) -> Result<Option<Entity>> {
        let entities = self.entities.read().await;
        Ok(entities.get(&req.id).cloned())
    }
    
    async fn query_entities(&self, req: QueryEntities) -> Result<QueryResult> {
        let entities = self.entities.read().await;
        
        let filtered: Vec<Entity> = entities
            .values()
            .filter(|e| {
                if let Some(et) = &req.entity_type {
                    &e.entity_type == et
                } else {
                    true
                }
            })
            .filter(|e| {
                if let Some(min) = req.aabb_min {
                    if let Some(max) = req.aabb_max {
                        e.position[0] >= min[0] && e.position[0] <= max[0] &&
                        e.position[1] >= min[1] && e.position[1] <= max[1]
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
            .filter(|e| {
                if let Some(owner_id) = req.owner_id {
                    e.metadata.iter()
                        .find(|(k, _)| k == "owner_id")
                        .and_then(|(_, v)| v.parse::<u64>().ok())
                        .map(|id| id == owner_id)
                        .unwrap_or(false)
                } else {
                    true
                }
            })
            .cloned()
            .collect();
        
        Ok(QueryResult { entities: filtered })
    }
    
    async fn join_game(&self, req: JoinGame) -> Result<PlayerSession> {
        let mut sessions = self.player_sessions.write().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        let session = PlayerSession {
            player_id: req.player_id,
            name: req.name,
            ship_id: None,
            camera_position: [0.0, 0.0],
            connected_at: now,
            last_seen: now,
        };
        
        sessions.insert(req.player_id, session.clone());
        
        Ok(session)
    }
    
    async fn leave_game(&self, req: LeaveGame) -> Result<()> {
        let mut sessions = self.player_sessions.write().await;
        sessions.remove(&req.player_id);
        Ok(())
    }
    
    async fn register_renderer(&self, _req: RegisterRenderer) -> Result<()> {
        // Force reconnection
        *self.renderer_client.lock().await = None;
        self.ensure_renderer().await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🌍 World Cell - Authoritative State Manager");
    println!("   └─ Source of Truth");
    println!("   └─ Entities: 0");
    println!("   └─ Players: 0");
    
    // Determine database path
    let home = dirs::home_dir().expect("No HOME directory");
    let db_dir = home.join(".cell/data/world");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("world.db");
    
    let service = WorldService {
        db: Arc::new(Mutex::new(Connection::open(&db_path)?)),
        db_path,
        entities: Arc::new(RwLock::new(HashMap::new())),
        renderer_client: Arc::new(Mutex::new(None)),
        next_entity_id: Arc::new(RwLock::new(1)),
        player_sessions: Arc::new(RwLock::new(HashMap::new())),
    };
    
    // Initialize database
    service.init_db()?;
    
    service.serve("world").await
}