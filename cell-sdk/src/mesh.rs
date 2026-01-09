// SPDX-License-Identifier: MIT
// cell-sdk/src/mesh.rs

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use tokio::sync::RwLock;
use tracing::info;

static DEPENDENCY_MAP: OnceLock<RwLock<HashMap<String, HashSet<String>>>> = OnceLock::new();

fn get_dependency_map() -> &'static RwLock<HashMap<String, HashSet<String>>> {
    DEPENDENCY_MAP.get_or_init(|| RwLock::new(HashMap::new()))
}

pub struct MeshBuilder;

impl MeshBuilder {
    pub async fn declare_dependencies(cell: &str, deps: Vec<String>) {
        let map = get_dependency_map();
        let mut guard = map.write().await;
        guard.insert(cell.to_string(), deps.into_iter().collect());
    }

    /// Announce presence by creating a lockfile or registry entry.
    pub async fn announce_self(name: &str) -> Result<()> {
        let cwd = std::env::current_dir()?;
        // In a shared registry model, we'd write to ~/.cell/registry/active
        // In this localized model, simply existing is enough.
        // But for "Discovery", we might write a PID file.
        let pid = std::process::id();
        let pid_file = cwd.join(".cell/pid");
        tokio::fs::write(pid_file, pid.to_string()).await.ok();

        info!("[Mesh] Announced '{}' (PID {})", name, pid);
        Ok(())
    }

    /// Blocks until dependency folders exist in ./.cell/neighbors/
    pub async fn wait_for_dependencies(deps: &[&str]) -> Result<()> {
        if deps.is_empty() {
            return Ok(());
        }

        let cwd = std::env::current_dir()?;
        let neighbors_dir = cwd.join(".cell/neighbors");

        info!("[Mesh] Waiting for neighbors: {:?}", deps);

        for &dep in deps {
            let dep_path = neighbors_dir.join(dep);
            let mut attempts = 0;

            loop {
                // We check if the neighbor directory exists.
                // Organogenesis creates these links.
                // But the target cell must have created its .cell/io/in file for the link to be valid.
                // Since we use symlinks, we check if the link points to a valid file.

                let tx_link = dep_path.join("tx");

                if tx_link.exists() {
                    // It exists!
                    info!("[Mesh] Neighbor '{}' is online.", dep);
                    break;
                }

                if attempts % 10 == 0 && attempts > 0 {
                    info!("[Mesh] Waiting for '{}'...", dep);
                }
                attempts += 1;
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        }
        Ok(())
    }
}
