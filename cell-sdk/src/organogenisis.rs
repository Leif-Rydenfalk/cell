// SPDX-License-Identifier: MIT
// cell-sdk/src/organogenesis.rs

use anyhow::{Context, Result};
use cell_model::manifest::CellManifest;
use std::fs;

pub struct Organism;

impl Organism {
    pub fn develop() -> Result<()> {
        let cwd = std::env::current_dir()?;
        let manifest_path = cwd.join("Cell.toml");

        if !manifest_path.exists() {
            fs::create_dir_all(cwd.join(".cell/io"))?;
            return Ok(());
        }

        let content = fs::read_to_string(&manifest_path)?;
        let manifest: CellManifest =
            toml::from_str(&content).context("Failed to parse Cell.toml")?;

        let runtime_dir = cwd.join(".cell");
        let neighbors_dir = runtime_dir.join("neighbors");
        let io_dir = runtime_dir.join("io");

        fs::create_dir_all(&neighbors_dir)?;
        fs::create_dir_all(&io_dir)?;

        for (name, config) in &manifest.neighbors {
            let rel_path_str = match config {
                cell_model::manifest::NeighborConfig::Path(p) => p,
                cell_model::manifest::NeighborConfig::Detailed { path, .. } => path,
            };

            let target_root = cwd.join(rel_path_str);

            // The target's socket is always at .cell/io/in relative to its CWD
            // BUT we need to know its CWD. For local paths, we can resolve it.
            let target_cwd = if rel_path_str.starts_with('.') || !rel_path_str.contains('/') {
                // Relative path from current cell to neighbor
                cwd.join(rel_path_str)
            } else {
                // Absolute or different relative - canonicalize
                fs::canonicalize(&target_root).unwrap_or(target_root)
            };

            let target_io = target_cwd.join(".cell/io");
            let link_dir = neighbors_dir.join(name);

            fs::create_dir_all(&link_dir)?;

            let my_tx_link = link_dir.join("tx");
            let target_in_socket = target_io.join("in");

            // Remove old link if exists
            if my_tx_link.exists() || fs::symlink_metadata(&my_tx_link).is_ok() {
                fs::remove_file(&my_tx_link).ok();
            }

            // Create symlink to target's expected socket location
            // Note: This might be a broken symlink until target actually binds
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target_in_socket, &my_tx_link).with_context(|| {
                format!(
                    "Failed to create neighbor link for '{}' -> {:?}",
                    name, target_in_socket
                )
            })?;

            tracing::info!(
                "[Organogenesis] Linked neighbor '{}' -> {:?}",
                name,
                target_in_socket
            );
        }

        Ok(())
    }
}
