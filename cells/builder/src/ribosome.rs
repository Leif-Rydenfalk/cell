// cells/builder/src/ribosome.rs
// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result, anyhow};
use fd_lock::RwLock;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use serde_json::Value;

pub struct Ribosome;

impl Ribosome {
    fn prepare_env(cell_name: &str) -> Result<(PathBuf, PathBuf)> {
        let home = dirs::home_dir().unwrap();
        let bin_dir = home.join(".cell/bin");
        let meta_dir = home.join(".cell/bin/.meta").join(cell_name);

        fs::create_dir_all(&bin_dir)?;
        fs::create_dir_all(&meta_dir)?;
        Ok((bin_dir, meta_dir))
    }

    pub fn synthesize(source_path: &Path, cell_name: &str) -> Result<(PathBuf, String)> {
        let (bin_dir, meta_dir) = Self::prepare_env(cell_name)?;
        
        let actual_source = fs::canonicalize(source_path).context("Failed to resolve source")?;
        
        // Compute Hash First
        let current_hash = Self::compute_dna_hash(&actual_source)?;
        let hash_file_path = meta_dir.join("dna.hash");
        let binary_path = bin_dir.join(cell_name);

        // Check Cache
        if binary_path.exists() && hash_file_path.exists() {
            let cached_hash = fs::read_to_string(&hash_file_path).unwrap_or_default();
            if cached_hash.trim() == current_hash {
                // Up to date
                return Ok((binary_path, current_hash));
            }
            tracing::info!("[Ribosome] Source change detected for '{}'. Re-synthesizing...", cell_name);
        } else {
            tracing::info!("[Ribosome] Synthesizing '{}'...", cell_name);
        }

        // Build
        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--release");
        Self::sanitize_cargo_cmd(&mut cmd);

        let status = cmd
            .current_dir(&actual_source)
            .env("CARGO_TARGET_DIR", &meta_dir.join("target"))
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run cargo build")?;

        if !status.success() {
            anyhow::bail!("Ribosome failed to compile {}", cell_name);
        }

        let artifact_name = if cfg!(windows) { format!("{}.exe", cell_name) } else { cell_name.to_string() };
        let built_binary = meta_dir.join("target/release").join(&artifact_name);
        
        if !built_binary.exists() {
            anyhow::bail!("Binary missing at {:?}", built_binary);
        }

        fs::copy(&built_binary, &binary_path)?;
        fs::write(&hash_file_path, &current_hash)?;
        
        Ok((binary_path, current_hash))
    }

    pub fn synthesize_test(source_path: &Path, cell_name: &str) -> Result<PathBuf> {
        let (_, meta_dir) = Self::prepare_env(cell_name)?;
        
        tracing::info!("[Ribosome] Compiling tests for '{}'...", cell_name);
        let actual_source = fs::canonicalize(source_path).context("Failed to resolve source")?;

        let mut cmd = Command::new("cargo");
        cmd.arg("test").arg("--no-run").arg("--message-format=json");
        Self::sanitize_cargo_cmd(&mut cmd);

        let output = cmd
            .current_dir(&actual_source)
            .env("CARGO_TARGET_DIR", &meta_dir.join("target"))
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Test compilation failed:\n{}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut test_binary = None;

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<Value>(line) {
                if let Some(reason) = json.get("reason").and_then(|s| s.as_str()) {
                    if reason == "compiler-artifact" {
                        if let Some(executable) = json.get("executable").and_then(|s| s.as_str()) {
                            if let Some(target) = json.get("target") {
                                let is_test = target.get("test").and_then(|b| b.as_bool()).unwrap_or(false);
                                if is_test {
                                    test_binary = Some(PathBuf::from(executable));
                                }
                            }
                        }
                    }
                }
            }
        }

        test_binary.ok_or_else(|| anyhow!("No test executable produced"))
    }

    fn compute_dna_hash(path: &Path) -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        
        // Hash critical build files
        let mut files = Vec::new();
        
        // Recursive directory walk
        let mut dirs = vec![path.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let name = p.file_name().unwrap_or_default();
                        if name != "target" && name != ".git" {
                            dirs.push(p);
                        }
                    } else if p.extension().map_or(false, |ext| ext == "rs" || ext == "toml" || ext == "lock") {
                        files.push(p);
                    }
                }
            }
        }
        
        files.sort(); // Deterministic order
        
        for f in files {
            if let Ok(bytes) = fs::read(&f) {
                hasher.update(&bytes);
            }
        }
        
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn sanitize_cargo_cmd(cmd: &mut Command) {
        for (key, _) in std::env::vars() {
            if key.starts_with("CARGO_") {
                cmd.env_remove(&key);
            }
        }
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        cmd.arg("--color=never");
    }
}