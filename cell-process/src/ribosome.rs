// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result};
use fd_lock::RwLock;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Ribosome;

impl Ribosome {
    pub fn synthesize(source_path: &Path, cell_name: &str) -> Result<PathBuf> {
        if cell_name.contains(&['/', '\\', '.'][..]) {
            anyhow::bail!("Invalid cell name: cannot contain path separators");
        }
        if cell_name.is_empty() || cell_name.len() > 100 {
            anyhow::bail!("Invalid cell name length");
        }

        // New Layout: ~/.cell/bin (was .cell/cache/proteins)
        let home = dirs::home_dir().unwrap();
        let bin_dir = home.join(".cell/bin");
        let meta_dir = home.join(".cell/bin/.meta").join(cell_name);

        fs::create_dir_all(&bin_dir)?;
        fs::create_dir_all(&meta_dir)?;

        let lock_path = meta_dir.join("ribosome.lock");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)?;

        let mut locker = RwLock::new(lock_file);
        let _guard = locker.write()?;

        // Resolve symlink to get actual source for hashing
        let actual_source = fs::canonicalize(source_path).context("Failed to resolve source path")?;
        
        let current_hash = Self::compute_dna_hash(&actual_source)?;
        
        // Binary name includes hash for content addressing? 
        // Or we keep simple name and check hash? Simple name is easier for Capsid.
        let binary_path = bin_dir.join(cell_name);
        let hash_file_path = meta_dir.join("dna.hash");

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

        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--release");

        if actual_source.join("vendor").exists() {
            cmd.arg("--offline");
        }

        let status = cmd
            .current_dir(&actual_source)
            .env("CARGO_TARGET_DIR", &meta_dir.join("target")) // Build artifacts in meta to keep bin clean
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run cargo build")?;

        if !status.success() {
            anyhow::bail!("Ribosome failed to compile {}", cell_name);
        }

        // Locate the artifact
        let built_binary = meta_dir.join("target/release").join(cell_name);
        if !built_binary.exists() {
            anyhow::bail!("Compiler finished but binary missing at {:?}", built_binary);
        }

        // Install to bin
        fs::copy(&built_binary, &binary_path)?;
        fs::write(&hash_file_path, current_hash)?;

        Ok(binary_path)
    }

    fn compute_dna_hash(path: &Path) -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        let mut files = Vec::new();

        let rustc_version = Command::new("rustc")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        hasher.update(rustc_version.as_bytes());

        let lockfile = path.join("Cargo.lock");
        if lockfile.exists() {
             if let Ok(content) = fs::read(&lockfile) {
                 hasher.update(&content);
             }
        }

        fn visit_dirs(dir: &Path, cb: &mut dyn FnMut(&Path)) -> std::io::Result<()> {
            if dir.is_dir() {
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();

                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
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