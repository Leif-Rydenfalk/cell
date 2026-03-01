//! Orbital Mechanics Cell - Pure physics engine
//! NOW: Directly pushes transforms to Renderer at 60Hz
//!
//! This cell:
//! 1. Computes celestial body positions using Keplerian elements
//! 2. Provides RPC API for body queries
//! 3. PUSHES transforms directly to Renderer @ 60Hz
//! 4. No World cell latency!

use anyhow::Result;
use cell_sdk::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

// ========= PROTEINS (PUBLIC API) =========
#[protein]
pub struct CelestialBody {
    pub id: String,
    pub name: String,
    pub body_type: BodyType,
    pub mass: f32,
    pub radius: f32,
    pub color: [f32; 3],
    
    // Keplerian elements
    pub semi_major_axis: f32,     // a
    pub eccentricity: f32,        // e
    pub inclination: f32,         // i (rad)
    pub longitude_ascending: f32, // Ω (rad)
    pub argument_periapsis: f32,  // ω (rad)
    pub mean_anomaly_epoch: f32,  // M0 (rad)
    pub period: f32,              // orbital period (seconds)
    
    // Current state
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub rotation: f32,
}

#[protein]
pub enum BodyType {
    Star,
    Planet,
    Moon,
    Dwarf,
    Asteroid,
}

#[protein]
pub struct AddBody {
    pub body: CelestialBody,
}

#[protein]
pub struct RemoveBody {
    pub id: String,
}

#[protein]
pub struct GetBodies;

#[protein]
pub struct BodiesList {
    pub bodies: Vec<CelestialBody>,
}

#[protein]
pub struct GetBody {
    pub id: String,
}

#[protein]
pub struct UpdatePositions {
    pub delta_seconds: f32,
}

#[protein]
pub struct SetTimeScale {
    pub scale: f32,
}

#[protein]
pub struct GetTimeScale;

// ========= RENDERER LINK =========
// Direct connection to renderer for transform pushes

cell_remote!(Renderer = "renderer");

struct RendererLink {
    client: Arc<tokio::sync::Mutex<Option<Renderer::Client>>>,
    entity_prefix: String,
}

impl RendererLink {
    fn new() -> Self {
        Self {
            client: Arc::new(tokio::sync::Mutex::new(None)),
            entity_prefix: "body_".to_string(),
        }
    }
    
    async fn ensure_connected(&self) -> Result<Renderer::Client> {
        let mut client_guard = self.client.lock().await;
        
        if let Some(client) = client_guard.as_ref() {
            // Assume connected if present (ResilientSynapse handles reconnection)
            return Ok(client.clone());
        }
        
        // Reconnect
        match Renderer::Client::connect().await {
            Ok(client) => {
                tracing::info!("[Orbital] ✅ Connected to Renderer for transform pushes");
                *client_guard = Some(client.clone());
                Ok(client)
            }
            Err(e) => {
                anyhow::bail!("Failed to connect to Renderer: {}", e);
            }
        }
    }
    
    async fn push_batch(&self, transforms: Vec<(String, [f32; 16])>) -> Result<()> {
        let client = match self.ensure_connected().await {
            Ok(c) => c,
            Err(_) => return Ok(()), // Silently fail - will retry next tick
        };
        
        let updates = transforms.into_iter()
            .map(|(id, transform)| Renderer::TransformUpdate {
                entity_id: format!("{}{}", self.entity_prefix, id),
                transform,
            })
            .collect();
        
        match client.batch_update_transforms(Renderer::BatchUpdateTransforms { updates }).await {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::debug!("[Orbital] Failed to push transforms: {}", e);
                // Force reconnect next time
                *self.client.lock().await = None;
                Ok(())
            }
        }
    }
}

// ========= ORBITAL ENGINE =========
struct OrbitalEngine {
    bodies: HashMap<String, CelestialBody>,
    time_scale: f32,
    simulation_time: f32,
    last_update: Instant,
    g_constant: f32,
    renderer_link: Arc<RendererLink>,
}

impl OrbitalEngine {
    fn new() -> Self {
        let mut engine = Self {
            bodies: HashMap::new(),
            time_scale: 1.0,
            simulation_time: 0.0,
            last_update: Instant::now(),
            g_constant: 0.001, // Scaled for visual appeal
            renderer_link: Arc::new(RendererLink::new()),
        };
        
        engine.init_solar_system();
        engine
    }
    
    fn init_solar_system(&mut self) {
        // Sun
        self.bodies.insert("sun".to_string(), CelestialBody {
            id: "sun".to_string(),
            name: "Sun".to_string(),
            body_type: BodyType::Star,
            mass: 1000.0,
            radius: 2.0,
            color: [1.0, 0.8, 0.2],
            semi_major_axis: 0.0,
            eccentricity: 0.0,
            inclination: 0.0,
            longitude_ascending: 0.0,
            argument_periapsis: 0.0,
            mean_anomaly_epoch: 0.0,
            period: 0.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Mercury
        self.bodies.insert("mercury".to_string(), CelestialBody {
            id: "mercury".to_string(),
            name: "Mercury".to_string(),
            body_type: BodyType::Planet,
            mass: 0.055,
            radius: 0.4,
            color: [0.7, 0.7, 0.7],
            semi_major_axis: 4.0,
            eccentricity: 0.2056,
            inclination: 0.122,
            longitude_ascending: 0.843,
            argument_periapsis: 0.485,
            mean_anomaly_epoch: 0.0,
            period: 8.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Venus
        self.bodies.insert("venus".to_string(), CelestialBody {
            id: "venus".to_string(),
            name: "Venus".to_string(),
            body_type: BodyType::Planet,
            mass: 0.815,
            radius: 0.5,
            color: [1.0, 0.9, 0.6],
            semi_major_axis: 6.0,
            eccentricity: 0.0068,
            inclination: 0.059,
            longitude_ascending: 1.338,
            argument_periapsis: 0.957,
            mean_anomaly_epoch: 0.0,
            period: 12.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Earth
        self.bodies.insert("earth".to_string(), CelestialBody {
            id: "earth".to_string(),
            name: "Earth".to_string(),
            body_type: BodyType::Planet,
            mass: 1.0,
            radius: 0.6,
            color: [0.2, 0.6, 1.0],
            semi_major_axis: 8.0,
            eccentricity: 0.0167,
            inclination: 0.0,
            longitude_ascending: 0.0,
            argument_periapsis: 1.796,
            mean_anomaly_epoch: 0.0,
            period: 16.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Mars
        self.bodies.insert("mars".to_string(), CelestialBody {
            id: "mars".to_string(),
            name: "Mars".to_string(),
            body_type: BodyType::Planet,
            mass: 0.107,
            radius: 0.5,
            color: [1.0, 0.5, 0.2],
            semi_major_axis: 10.0,
            eccentricity: 0.0934,
            inclination: 0.032,
            longitude_ascending: 0.865,
            argument_periapsis: 5.865,
            mean_anomaly_epoch: 0.0,
            period: 20.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Jupiter
        self.bodies.insert("jupiter".to_string(), CelestialBody {
            id: "jupiter".to_string(),
            name: "Jupiter".to_string(),
            body_type: BodyType::Planet,
            mass: 317.8,
            radius: 1.2,
            color: [1.0, 0.7, 0.4],
            semi_major_axis: 14.0,
            eccentricity: 0.0489,
            inclination: 0.023,
            longitude_ascending: 1.754,
            argument_periapsis: 4.781,
            mean_anomaly_epoch: 0.0,
            period: 28.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Saturn
        self.bodies.insert("saturn".to_string(), CelestialBody {
            id: "saturn".to_string(),
            name: "Saturn".to_string(),
            body_type: BodyType::Planet,
            mass: 95.2,
            radius: 1.0,
            color: [0.9, 0.8, 0.5],
            semi_major_axis: 18.0,
            eccentricity: 0.0565,
            inclination: 0.047,
            longitude_ascending: 1.983,
            argument_periapsis: 5.864,
            mean_anomaly_epoch: 0.0,
            period: 36.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Uranus
        self.bodies.insert("uranus".to_string(), CelestialBody {
            id: "uranus".to_string(),
            name: "Uranus".to_string(),
            body_type: BodyType::Planet,
            mass: 14.5,
            radius: 0.8,
            color: [0.5, 0.8, 0.9],
            semi_major_axis: 22.0,
            eccentricity: 0.0461,
            inclination: 0.014,
            longitude_ascending: 0.408,
            argument_periapsis: 2.984,
            mean_anomaly_epoch: 0.0,
            period: 44.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Neptune
        self.bodies.insert("neptune".to_string(), CelestialBody {
            id: "neptune".to_string(),
            name: "Neptune".to_string(),
            body_type: BodyType::Planet,
            mass: 17.1,
            radius: 0.8,
            color: [0.3, 0.4, 0.9],
            semi_major_axis: 26.0,
            eccentricity: 0.0097,
            inclination: 0.030,
            longitude_ascending: 2.299,
            argument_periapsis: 4.632,
            mean_anomaly_epoch: 0.0,
            period: 52.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Pluto
        self.bodies.insert("pluto".to_string(), CelestialBody {
            id: "pluto".to_string(),
            name: "Pluto".to_string(),
            body_type: BodyType::Dwarf,
            mass: 0.0022,
            radius: 0.3,
            color: [0.8, 0.7, 0.6],
            semi_major_axis: 30.0,
            eccentricity: 0.2488,
            inclination: 0.299,
            longitude_ascending: 1.924,
            argument_periapsis: 4.141,
            mean_anomaly_epoch: 0.0,
            period: 60.0,
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            rotation: 0.0,
        });
        
        // Initial positions
        self.update_all_positions(0.0);
    }
    
    fn update_all_positions(&mut self, time: f32) {
        let sun_pos = [0.0, 0.0, 0.0];
        
        // Clone body data first to avoid double borrow
        let bodies: Vec<(String, f32, f32, f32, f32, f32, f32, f32)> = self.bodies
            .iter()
            .filter(|(id, _)| *id != "sun")
            .map(|(id, body)| (
                id.clone(),
                body.semi_major_axis,
                body.eccentricity,
                body.inclination,
                body.longitude_ascending,
                body.argument_periapsis,
                body.mean_anomaly_epoch,
                body.period,
            ))
            .collect();
        
        // Update sun
        if let Some(sun) = self.bodies.get_mut("sun") {
            sun.position = sun_pos;
        }
        
        // Update other bodies
        for (id, a, e, i, omega, w, m0, period) in bodies {
            let position = self.solve_kepler_params(a, e, i, omega, w, m0, period, time);
            if let Some(body) = self.bodies.get_mut(&id) {
                body.position = position;
            }
        }
    }
    
    fn solve_kepler_params(&self, 
        a: f32, e: f32, i: f32, omega: f32, w: f32, m0: f32, period: f32, time: f32
    ) -> [f32; 3] {
        if a == 0.0 {
            return [0.0, 0.0, 0.0];
        }
        
        // Mean anomaly
        let n = 2.0 * std::f32::consts::PI / period;
        let m = m0 + n * time;
        
        // Solve for eccentric anomaly E (Newton's method)
        let mut e_anomaly = m;
        for _ in 0..10 {
            let delta = (e_anomaly - e * e_anomaly.sin() - m) / (1.0 - e * e_anomaly.cos());
            e_anomaly -= delta;
            if delta.abs() < 1e-6 {
                break;
            }
        }
        
        // True anomaly
        let v = 2.0 * ((1.0 + e).sqrt() / (1.0 - e).sqrt() * (e_anomaly / 2.0).tan()).atan();
        
        // Distance from central body
        let r = a * (1.0 - e * e) / (1.0 + e * v.cos());
        
        // Position in orbital plane
        let x_orb = r * v.cos();
        let y_orb = r * v.sin();
        
        // Rotate to 3D space
        let cos_omega = omega.cos();
        let sin_omega = omega.sin();
        let cos_i = i.cos();
        let sin_i = i.sin();
        let cos_w = w.cos();
        let sin_w = w.sin();
        
        let x = (cos_w * cos_omega - sin_w * sin_omega * cos_i) * x_orb +
                (-sin_w * cos_omega - cos_w * sin_omega * cos_i) * y_orb;
        let y = (cos_w * sin_omega + sin_w * cos_omega * cos_i) * x_orb +
                (-sin_w * sin_omega + cos_w * cos_omega * cos_i) * y_orb;
        let z = (sin_w * sin_i) * x_orb + (cos_w * sin_i) * y_orb;
        
        [x, y, z]
    }
    
    fn update(&mut self, delta_seconds: f32) -> Vec<CelestialBody> {
        let dt = delta_seconds * self.time_scale;
        self.simulation_time += dt;
        
        self.update_all_positions(self.simulation_time);
        
        let bodies: Vec<CelestialBody> = self.bodies.values().cloned().collect();
        
        // Push transforms directly to renderer
        let renderer_link = self.renderer_link.clone();
        let bodies_for_push = bodies.clone();
        
        tokio::spawn(async move {
            let mut transforms = Vec::new();
            for body in bodies_for_push {
                let scale = if body.id == "sun" {
                    body.radius * 2.0
                } else {
                    body.radius * 3.0
                };
                
                let transform = Self::create_transform_matrix(body.position, scale);
                transforms.push((body.id, transform));
            }
            
            if let Err(e) = renderer_link.push_batch(transforms).await {
                tracing::debug!("[Orbital] Transform push failed: {}", e);
            }
        });
        
        bodies
    }
    
    fn create_transform_matrix(position: [f32; 3], scale: f32) -> [f32; 16] {
        [
            scale, 0.0,   0.0,   0.0,
            0.0,   scale, 0.0,   0.0,
            0.0,   0.0,   scale, 0.0,
            position[0], position[1], position[2], 1.0,
        ]
    }
}

// ========= SERVICE =========
#[service]
#[derive(Clone)]
struct OrbitalService {
    engine: Arc<RwLock<OrbitalEngine>>,
    // Add set to track what we've spawned
    spawned_bodies: Arc<RwLock<HashSet<String>>>,
}

#[handler]
impl OrbitalService {
    async fn add_body(&self, req: AddBody) -> Result<()> {
        let mut engine = self.engine.write().unwrap();
        engine.bodies.insert(req.body.id.clone(), req.body);
        tracing::info!("[Orbital] Added body");
        Ok(())
    }
    
    async fn remove_body(&self, req: RemoveBody) -> Result<()> {
        let mut engine = self.engine.write().unwrap();
        engine.bodies.remove(&req.id);
        Ok(())
    }
    
    async fn get_bodies(&self, _req: GetBodies) -> Result<BodiesList> {
        let engine = self.engine.read().unwrap();
        Ok(BodiesList {
            bodies: engine.bodies.values().cloned().collect(),
        })
    }
    
    async fn get_body(&self, req: GetBody) -> Result<Option<CelestialBody>> {
        let engine = self.engine.read().unwrap();
        Ok(engine.bodies.get(&req.id).cloned())
    }
    
    async fn update_positions(&self, req: UpdatePositions) -> Result<Vec<CelestialBody>> {
        let mut engine = self.engine.write().unwrap();
        Ok(engine.update(req.delta_seconds))
    }
    
    async fn set_time_scale(&self, req: SetTimeScale) -> Result<()> {
        let mut engine = self.engine.write().unwrap();
        engine.time_scale = req.scale;
        tracing::info!("[Orbital] Time scale set to {}", req.scale);
        Ok(())
    }
    
    async fn get_time_scale(&self, _req: GetTimeScale) -> Result<f32> {
        let engine = self.engine.read().unwrap();
        Ok(engine.time_scale)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("🪐 Orbital Cell - Direct Renderer Push Mode");
    println!("   └─ Computes Keplerian orbital mechanics");
    println!("   └─ Provides RPC API for body queries");
    println!("   └─ ✅ PUSHES transforms directly to Renderer @ 60Hz");
    println!("   └─ ❌ NO World cell latency!");
    println!("   └─ 📦 Batch updates - 1 RPC per frame");
    
    let service = OrbitalService {
        engine: Arc::new(RwLock::new(OrbitalEngine::new())),
        spawned_bodies: Arc::new(RwLock::new(HashSet::new())),
    };
    
    // Spawn physics ticker (60Hz)
    let service_clone = service.clone();
    let mut last_time = std::time::Instant::now();
    
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(16)); // 60Hz
        
        // Wait for renderer to be ready
        tokio::time::sleep(Duration::from_secs(2)).await;
        
        loop {
            interval.tick().await;
            let delta = last_time.elapsed().as_secs_f32();
            last_time = std::time::Instant::now();
            
            // Get updated bodies and spawn visual entities if needed
            if let Ok(bodies) = service_clone.update_positions(UpdatePositions { delta_seconds: delta }).await {
                // Check spawning in a separate scope to avoid long locks
                let bodies_to_spawn: Vec<CelestialBody> = {
                    let spawned = service_clone.spawned_bodies.read().unwrap();
                    bodies.iter()
                        .filter(|b| !spawned.contains(&b.id))
                        .cloned()
                        .collect()
                };
                
                if !bodies_to_spawn.is_empty() {
                    // Extract link in a block to drop the lock guard immediately
                    let renderer_link = {
                        let engine = service_clone.engine.read().unwrap();
                        engine.renderer_link.clone()
                    }; 
                    
                    if let Ok(client) = renderer_link.ensure_connected().await {
                        for body in bodies_to_spawn {
                            tracing::info!("[Orbital] Spawning visual for {}", body.name);
                            let scale = if body.id == "sun" { body.radius * 2.0 } else { body.radius * 3.0 };
                            
                            // We use a cube for now as placeholder
                            // In real game we would have sphere meshes
                            let _ = client.spawn_entity(Renderer::SpawnEntity {
                                entity_id: format!("body_{}", body.id),
                                pass_id: "ship".to_string(), // Re-use ship pass for now
                                buffer_id: "cube".to_string(),
                                vertex_count: 36,
                                instance_count: 1,
                                transform: [
                                    scale, 0.0, 0.0, 0.0,
                                    0.0, scale, 0.0, 0.0,
                                    0.0, 0.0, scale, 0.0,
                                    body.position[0], body.position[1], body.position[2], 1.0,
                                ],
                            }).await;
                            
                            service_clone.spawned_bodies.write().unwrap().insert(body.id);
                        }
                    }
                }
            }
        }
    });
    
    // Start RPC server
    service.serve("orbital").await
}