// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::Synapse;
use anyhow::{Result, Context};
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;
use tracing::{info, warn};
use std::sync::OnceLock;
use crate::identity::Identity;

static DEPENDENCY_MAP: OnceLock<RwLock<HashMap<String, HashSet<String>>>> = OnceLock::new();

fn get_dependency_map() -> &'static RwLock<HashMap<String, HashSet<String>>> {
    DEPENDENCY_MAP.get_or_init(|| RwLock::new(HashMap::new()))
}

pub struct MeshBuilder;

impl MeshBuilder {
    /// Declare dependencies for a cell (runtime registry)
    pub async fn declare_dependencies(cell: &str, deps: Vec<String>) {
        let map = get_dependency_map();
        let mut guard = map.write().await;
        guard.insert(cell.to_string(), deps.into_iter().collect());
    }

    /// Wait for all dependencies to be reachable before proceeding.
    /// Also reports these dependencies to the central Mesh service for graph analysis.
    pub async fn wait_for_dependencies(deps: &[&str]) -> Result<()> {
        if deps.is_empty() {
            return Ok(());
        }

        // 1. Report to Mesh Service (Best Effort)
        let self_name = Identity::get().cell_name.clone();
        let deps_vec: Vec<String> = deps.iter().map(|s| s.to_string()).collect();
        
        // Spawn report task to avoid blocking boot on non-critical Mesh service availability
        tokio::spawn(async move {
            // Avoid recursion if we ARE the mesh cell
            if self_name == "mesh" { return; }

            // Connect to Mesh service
            // We use a raw connection to avoid importing generated clients here
            match Synapse::grow("mesh").await {
                Ok(mut synapse) => {
                    let req = cell_model::protocol::MeshRequest::ResolveDependencies {
                        cell_name: self_name.clone(),
                        dependencies: deps_vec,
                    };
                    
                    if let Err(e) = synapse.fire::<cell_model::protocol::MeshRequest, cell_model::protocol::MeshResponse>(&req).await {
                        warn!("[Mesh] Failed to report dependencies: {}", e);
                    }
                }
                Err(e) => {
                    // It's okay if Mesh isn't running yet, we just can't report graph edges.
                    // GC might be aggressive but system will function.
                    warn!("[Mesh] Could not connect to Mesh service: {}", e);
                }
            }
        });

        info!("[Mesh] Waiting for dependencies: {:?}", deps);

        for &dep in deps {
            // Poll until dependency is reachable
            let mut attempts = 0;
            loop {
                match Synapse::grow(dep).await {
                    Ok(_) => {
                        info!("[Mesh] Dependency '{}' is ready.", dep);
                        break;
                    }
                    Err(_) => {
                        if attempts % 20 == 0 && attempts > 0 {
                            info!("[Mesh] Waiting for '{}'...", dep);
                        }
                        attempts += 1;
                        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                    }
                }
            }
        }
        Ok(())
    }
}