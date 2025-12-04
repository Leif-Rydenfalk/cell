// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result};
use fd_lock::RwLock;
use std::fs::{self, OpenOptions}; // Removed unused 'File'
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Ribosome;

impl Ribosome {
    pub fn synthesize(source_path: &Path, cell_name: &str) -> Result<PathBuf> {
        // Fix #11: Path Traversal
        if cell_name.contains(&['/', '\\', '.'][..]) {
            anyhow::bail!("Invalid cell name: cannot contain path separators");
        }
        if cell_name.is_empty() || cell_name.len() > 100 {
            anyhow::bail!("Invalid cell name length");
        }

        let cache_dir = dirs::home_dir().unwrap().join(".cell/cache");
        let protein_dir = cache_dir.join("proteins").join(cell_name);

        fs::create_dir_all(&protein_dir)?;

        let lock_path = protein_dir.join("ribosome.lock");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)?;

        let mut locker = RwLock::new(lock_file);
        let _guard = locker.write()?;

        let binary_path = protein_dir.join("release").join(cell_name);
        let hash_file_path = protein_dir.join("dna.hash");

        let current_hash = Self::compute_dna_hash(source_path)?;

        if binary_path.exists() && hash_file_path.exists() {
            let cached_hash = fs::read_to_string(&hash_file_path).unwrap_or_default();
            if cached_hash.trim() == current_hash {
                return Ok(binary_path);
            } else {
                println!(
                    "[Ribosome] Mutation detected in '{}'. Re-synthesizing...",
                    cell_name
                );
            }
        } else {
            println!("[Ribosome] Synthesizing '{}'...", cell_name);
        }

        if !source_path.join("vendor").exists() {
            // Only warn once per session to reduce noise?
            // For now, we keep it but it's fine.
        }

        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--release");

        if source_path.join("vendor").exists() {
            cmd.arg("--offline");
        }

        let status = cmd
            .current_dir(source_path)
            .env("CARGO_TARGET_DIR", &protein_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run cargo build")?;

        if !status.success() {
            anyhow::bail!("Ribosome failed to compile {}", cell_name);
        }

        if !binary_path.exists() {
            anyhow::bail!("Compiler finished but binary missing at {:?}", binary_path);
        }

        fs::write(&hash_file_path, current_hash)?;

        Ok(binary_path)
    }

    fn compute_dna_hash(path: &Path) -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        let mut files = Vec::new();

        fn visit_dirs(dir: &Path, cb: &mut dyn FnMut(&Path)) -> std::io::Result<()> {
            if dir.is_dir() {
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();

                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        // FIX: Ignore Cargo.lock to prevent rebuild loops when lockfile updates
                        if name == "target" || name.starts_with('.') || name == "Cargo.lock" {
                            continue;
                        }
                    }

                    if path.is_dir() {
                        visit_dirs(&path, cb)?;
                    } else {
                        cb(&path);
                    }
                }
            }
            Ok(())
        }

        visit_dirs(path, &mut |p| files.push(p.to_path_buf()))?;
        files.sort();

        for file_path in files {
            let bytes = fs::read(&file_path)?;
            hasher.update(&bytes);
        }

        Ok(hasher.finalize().to_hex().to_string())
    }
}