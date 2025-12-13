// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use crate::Synapse;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;
use tracing::info;

static DEPENDENCY_MAP: RwLock<HashMap<String, HashSet<String>>> = RwLock::new(HashMap::new());

pub struct MeshBuilder;

impl MeshBuilder {
    /// Declare dependencies for a cell (runtime registry)
    pub async fn declare_dependencies(cell: &str, deps: Vec<String>) {
        let mut map = DEPENDENCY_MAP.write().await;
        map.insert(cell.to_string(), deps.into_iter().collect());
    }

    /// Wait for all dependencies to be reachable before proceeding.
    /// This effectively implements the runtime mesh construction synchronization.
    pub async fn wait_for_dependencies(deps: &[&str]) -> Result<()> {
        if deps.is_empty() {
            return Ok(());
        }

        info!("[Mesh] Waiting for dependencies: {:?}", deps);

        for &dep in deps {
            // Poll until dependency is reachable
            let mut attempts = 0;
            loop {
                // We use Synapse::grow because it handles discovery and connection logic.
                // If Mycelium (via cell_remote logic) has ensured it exists, this checks readiness.
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