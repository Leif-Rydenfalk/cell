// cells/builder/src/main.rs
// SPDX-License-Identifier: MIT
// The Ribosome: Compiles DNA (Source) into Proteins (Binaries)

use cell_sdk::*;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs::{self, OpenOptions};
use fd_lock::RwLock;
use tracing::{info, warn, error};

#[protein]
pub struct BuildRequest {
    pub cell_name: String,
}

#[protein]
pub struct BuildResponse {
    pub binary_path: String,
}

struct BuilderService {
    registry_path: PathBuf,
    bin_dir: PathBuf,
}

impl BuilderService {
    fn new() -> Self {
        let home = dirs::home_dir().expect("No HOME dir");
        // Env overrides for testing/isolation
        let registry_path = std::env::var("CELL_REGISTRY_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".cell/registry"));
        
        let bin_dir = home.join(".cell/bin");
        
        fs::create_dir_all(&bin_dir).ok();
        
        Self {
            registry_path,
            bin_dir,
        }
    }

    fn synthesize(&self, cell_name: &str) -> Result<PathBuf> {
        if cell_name.contains(&['/', '\\', '.'][..]) {
            anyhow::bail!("Invalid cell name");
        }

        let source_path = self.registry_path.join(cell_name);
        if !source_path.exists() {
            anyhow::bail!("Cell '{}' not found in registry at {:?}", cell_name, source_path);
        }

        let meta_dir = self.bin_dir.join(".meta").join(cell_name);
        fs::create_dir_all(&meta_dir)?;

        let lock_path = meta_dir.join("ribosome.lock");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)?;

        let mut locker = RwLock::new(lock_file);
        let _guard = locker.write()?;

        // Resolve symlink
        let actual_source = fs::canonicalize(&source_path).context("Failed to resolve source path")?;
        
        let current_hash = self.compute_dna_hash(&actual_source)?;
        let binary_path = self.bin_dir.join(cell_name);
        let hash_file_path = meta_dir.join("dna.hash");

        if binary_path.exists() && hash_file_path.exists() {
            let cached_hash = fs::read_to_string(&hash_file_path).unwrap_or_default();
            if cached_hash.trim() == current_hash {
                return Ok(binary_path);
            }
            info!("[Builder] Source changed for '{}'. Recompiling...", cell_name);
        } else {
            info!("[Builder] Compiling '{}'...", cell_name);
        }

        // Sanitize Environment to prevent "Cargo Inception"
        let mut cmd = Command::new("cargo");
        cmd.arg("build").arg("--release");
        
        // Strip CARGO_* env vars inherited from parent process (e.g. if running via cargo test)
        for (key, _) in std::env::vars() {
            if key.starts_with("CARGO_") {
                cmd.env_remove(&key);
            }
        }

        // Restore PATH needed for rustc
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }

        if actual_source.join("vendor").exists() {
            cmd.arg("--offline");
        }

        let status = cmd
            .current_dir(&actual_source)
            .env("CARGO_TARGET_DIR", &meta_dir.join("target"))
            .stdout(std::process::Stdio::inherit()) // Pipe to stdout for visibility
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run cargo build")?;

        if !status.success() {
            anyhow::bail!("Compilation failed for {}", cell_name);
        }

        // Find artifact
        // Note: This assumes standard cargo output structure
        let artifact_name = if cfg!(windows) { format!("{}.exe", cell_name) } else { cell_name.to_string() };
        let built_binary = meta_dir.join("target/release").join(&artifact_name);
        
        if !built_binary.exists() {
            anyhow::bail!("Binary missing at {:?}", built_binary);
        }

        fs::copy(&built_binary, &binary_path)?;
        fs::write(&hash_file_path, current_hash)?;

        info!("[Builder] Successfully built '{}'", cell_name);
        Ok(binary_path)
    }

    fn compute_dna_hash(&self, path: &Path) -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        
        // Include rustc version
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

        // Deep walk source
        let mut files = Vec::new();
        let src_dir = path.join("src");
        if src_dir.exists() {
            self.visit_dirs(&src_dir, &mut files)?;
        }
        
        // Include Cargo.toml
        files.push(path.join("Cargo.toml"));

        files.sort();

        for p in files {
            if let Ok(bytes) = fs::read(&p) {
                hasher.update(&bytes);
            }
        }

        Ok(hasher.finalize().to_hex().to_string())
    }

    fn visit_dirs(&self, dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if dir.is_dir() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    self.visit_dirs(&path, files)?;
                } else {
                    files.push(path);
                }
            }
        }
        Ok(())
    }
}

#[service]
#[derive(Clone)]
struct Builder {
    svc: std::sync::Arc<BuilderService>,
}

#[handler]
impl Builder {
    async fn build(&self, req: BuildRequest) -> Result<BuildResponse> {
        let path = self.svc.synthesize(&req.cell_name)?;
        Ok(BuildResponse {
            binary_path: path.to_string_lossy().to_string(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    info!("[Builder] Compiler Active");
    let service = Builder { svc: std::sync::Arc::new(BuilderService::new()) };
    service.serve("builder").await
}