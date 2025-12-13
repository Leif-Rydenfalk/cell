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

    pub fn synthesize(source_path: &Path, cell_name: &str) -> Result<PathBuf> {
        let (bin_dir, meta_dir) = Self::prepare_env(cell_name)?;
        
        // Simplified cache check for standard builds
        let binary_path = bin_dir.join(cell_name);
        // (Hashing logic omitted for brevity, keeping it simple for now)
        
        tracing::info!("[Ribosome] Synthesizing '{}'...", cell_name);

        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--release");
        Self::sanitize_cargo_cmd(&mut cmd);

        let actual_source = fs::canonicalize(source_path).context("Failed to resolve source")?;
        
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
        Ok(binary_path)
    }

    pub fn synthesize_test(source_path: &Path, cell_name: &str) -> Result<PathBuf> {
        let (_, meta_dir) = Self::prepare_env(cell_name)?;
        
        tracing::info!("[Ribosome] Compiling tests for '{}'...", cell_name);

        let actual_source = fs::canonicalize(source_path).context("Failed to resolve source")?;

        let mut cmd = Command::new("cargo");
        cmd.arg("test")
           .arg("--no-run")
           .arg("--message-format=json");
        
        Self::sanitize_cargo_cmd(&mut cmd);

        let output = cmd
            .current_dir(&actual_source)
            .env("CARGO_TARGET_DIR", &meta_dir.join("target"))
            .output()
            .context("Failed to run cargo test build")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Test compilation failed:\n{}", stderr);
        }

        // Parse JSON output to find the test executable
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut test_binary = None;

        for line in stdout.lines() {
            if let Ok(json) = serde_json::from_str::<Value>(line) {
                if let Some(reason) = json.get("reason").and_then(|s| s.as_str()) {
                    if reason == "compiler-artifact" {
                        if let Some(executable) = json.get("executable").and_then(|s| s.as_str()) {
                            if let Some(target) = json.get("target") {
                                // We prefer integration tests ('test' kind) or bin tests
                                let is_test = target.get("test").and_then(|b| b.as_bool()).unwrap_or(false);
                                // let kind = target.get("kind").and_then(|k| k.as_array()).map(|a| a[0].as_str().unwrap_or(""));
                                
                                if is_test {
                                    test_binary = Some(PathBuf::from(executable));
                                    // Keep going to find the *last* one? Or break?
                                    // Usually there is one main integration test binary if there is a `tests/` dir.
                                    // If multiple, this picks one arbitrarily (the last one encountered).
                                }
                            }
                        }
                    }
                }
            }
        }

        test_binary.ok_or_else(|| anyhow!("No test executable produced by cargo"))
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
        // Force color off for cleaner logs
        cmd.arg("--color=never");
    }
}