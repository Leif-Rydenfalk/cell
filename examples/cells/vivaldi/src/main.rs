// cells/vivaldi/src/main.rs
// SPDX-License-Identifier: MIT
// Vivaldi Network Coordinates for Latency-Aware Routing

use cell_sdk::*;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// === PROTOCOL ===

#[protein]
pub struct Coordinate {
    pub vec: [f32; 3], // 3D Euclidian coords
    pub height: f32,   // Height above plane (min latency)
    pub error: f32,    // Error estimate
}

#[protein]
pub struct RoutingQuery {
    pub target_cell: String,
    pub source_coordinate: Option<Coordinate>,
    pub max_results: u32,
}

#[protein]
pub struct RoutingResult {
    pub instances: Vec<String>, // Sorted by estimated latency
}

#[protein]
pub struct UpdateRTT {
    pub node_id: String,
    pub rtt_ms: f32,
    pub peer_coordinate: Coordinate,
}

// === SERVICE ===

pub struct VivaldiService {
    // Map NodeID/Address -> Coordinate
    coordinates: Arc<RwLock<HashMap<String, Coordinate>>>,
}

impl VivaldiService {
    pub fn new() -> Self {
        Self {
            coordinates: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn distance(a: &Coordinate, b: &Coordinate) -> f32 {
        let dist = ((a.vec[0] - b.vec[0]).powi(2) + 
                   (a.vec[1] - b.vec[1]).powi(2) + 
                   (a.vec[2] - b.vec[2]).powi(2)).sqrt();
        dist + a.height + b.height
    }
}

#[handler]
impl VivaldiService {
    pub async fn update(&self, update: UpdateRTT) -> Result<Coordinate> {
        let mut coords = self.coordinates.write().await;
        
        let my_coord_entry = coords.entry("self".to_string()).or_insert(Coordinate {
            vec: [0.0, 0.0, 0.0],
            height: 0.1,
            error: 1.0,
        });

        // Copy values to avoid multiple mutable borrows or simultaneous borrow
        let mut my_coord = my_coord_entry.clone();

        // Vivaldi Update Logic (Simplified)
        let dist_est = Self::distance(&my_coord, &update.peer_coordinate);
        let error = (dist_est - update.rtt_ms).abs();
        
        // Update local error
        const CE: f32 = 0.25;
        my_coord.error = my_coord.error * (1.0 - CE) + error * CE;
        
        // Update local coordinate (Mass-spring)
        const CC: f32 = 0.25;
        let delta = CC * (error / dist_est); // Simple timestep
        
        // Apply force vector (omitted full math for brevity, just updating height as example)
        my_coord.height = (my_coord.height + delta).max(0.1);
        
        // Save back my_coord
        coords.insert("self".to_string(), my_coord.clone());
        
        // Store peer's coord
        coords.insert(update.node_id, update.peer_coordinate);

        Ok(Coordinate {
            vec: my_coord.vec,
            height: my_coord.height,
            error: my_coord.error,
        })
    }

    pub async fn route(&self, query: RoutingQuery) -> Result<RoutingResult> {
        // In a real impl, we'd query the DHT/Nucleus for candidates first
        // For now, we sort known coordinates
        let coords = self.coordinates.read().await;
        
        let source = query.source_coordinate.as_ref().unwrap_or_else(|| {
            coords.get("self").unwrap_or(&Coordinate { 
                vec: [0.0, 0.0, 0.0], height: 10.0, error: 1.0 
            })
        });

        let mut candidates: Vec<(String, f32)> = coords.iter()
            .filter(|(k, _)| k.contains(&query.target_cell)) // Naive filter
            .map(|(k, v)| (k.clone(), Self::distance(source, v)))
            .collect();

        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        
        let instances = candidates.into_iter()
            .take(query.max_results as usize)
            .map(|(k, _)| k)
            .collect();

        Ok(RoutingResult { instances })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let service = VivaldiService::new();
    tracing::info!("[Vivaldi] Network Coordinate System Active");
    service.serve("vivaldi").await
}