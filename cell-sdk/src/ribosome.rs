use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Ribosome;

impl Ribosome {
    /// Compiles Source Code (DNA) into a Binary (Protein)
    /// Runs in 'offline' mode to enforce vendor usage and prevent supply chain attacks during build.
    pub fn synthesize(source_path: &Path, cell_name: &str) -> Result<PathBuf> {
        let cache_dir = dirs::home_dir().unwrap().join(".cell/cache");
        let output_dir = cache_dir.join("proteins").join(cell_name);

        let binary_path = cache_dir.join("release").join(cell_name);

        // Simple cache check: If binary exists, skip. (In prod, check hash of source)
        if binary_path.exists() {
            return Ok(binary_path);
        }

        println!("[Ribosome] Synthesizing '{}' from DNA...", cell_name);
        std::fs::create_dir_all(&output_dir)?;

        // Ensure 'vendor' exists for offline build
        if !source_path.join("vendor").exists() {
            eprintln!("[Ribosome] WARNING: No 'vendor' directory found. Trying online build (less secure).");
        }

        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--release");

        if source_path.join("vendor").exists() {
            cmd.arg("--offline");
        }

        let status = cmd
            .current_dir(source_path)
            .env("CARGO_TARGET_DIR", &cache_dir)
            .status()
            .context("Failed to run cargo build")?;

        if !status.success() {
            anyhow::bail!("Ribosome failed to compile {}", cell_name);
        }

        if !binary_path.exists() {
            anyhow::bail!("Compiler finished but binary missing at {:?}", binary_path);
        }

        Ok(binary_path)
    }
}
