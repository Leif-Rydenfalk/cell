// cell-sdk/src/organogenesis.rs
// Bootstrapping the cell's physical structure from DNA (Cell.toml)

use anyhow::{Context, Result};
use cell_model::manifest::CellManifest;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub struct Organism {
    pub manifest: CellManifest,
    pub runtime_dir: PathBuf,
}

impl Organism {
    /// Reads DNA, builds cytoskeleton (.cell directory), connects Axons (symlinks)
    pub fn develop() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let manifest_path = cwd.join("Cell.toml");

        if !manifest_path.exists() {
            // Fallback for raw binaries without manifest
            return Ok(Self {
                manifest: CellManifest {
                    package: cell_model::manifest::PackageMeta {
                        name: "unknown".into(),
                        version: "0.0.0".into(),
                    },
                    neighbors: std::collections::HashMap::new(),
                },
                runtime_dir: cwd.join(".cell"),
            });
        }

        let content = fs::read_to_string(&manifest_path)?;
        let manifest: CellManifest =
            toml::from_str(&content).context("Failed to parse Cell.toml")?;

        info!("🧬 Organogenesis: {}", manifest.package.name);

        // 1. Create Runtime Directory (.cell)
        let runtime_dir = cwd.join(".cell");
        if !runtime_dir.exists() {
            fs::create_dir(&runtime_dir)?;
        }

        // 2. Create Neighbors Directory
        let neighbors_dir = runtime_dir.join("neighbors");
        if !neighbors_dir.exists() {
            fs::create_dir(&neighbors_dir)?;
        }

        // 3. Link Neighbors (Grow Axons)
        for (name, rel_path) in &manifest.neighbors {
            let target_path = cwd.join(rel_path);
            let target_socket = target_path.join(".cell/me.sock");
            let link_path = neighbors_dir.join(name);

            // Clean old link
            if link_path.exists() || link_path.is_symlink() {
                fs::remove_file(&link_path).ok();
            }

            // We link to where the socket WILL be.
            // Note: We don't link to the socket file directly because it might not exist yet.
            // We link to the directory or handle the path resolution at runtime.
            // Actually, symlinking the socket path is standard for Unix.
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                // We create a broken symlink if the target doesn't exist yet. That's fine.
                // Mitosis will handle the wake-up.
                if let Err(e) = symlink(&target_socket, &link_path) {
                    warn!("Failed to link neighbor '{}': {}", name, e);
                } else {
                    info!("🔗 Linked neighbor '{}' -> {:?}", name, target_path);
                }
            }
        }

        Ok(Self {
            manifest,
            runtime_dir,
        })
    }
}
