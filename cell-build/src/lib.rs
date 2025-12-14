// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use syn::visit_mut::VisitMut;
use syn::{parse_file, Item};
use walkdir::WalkDir;

// --- PROTOCOL ---
#[derive(Serialize, Deserialize, Debug)]
pub enum ResolverRequest {
    EnsureRunning { cell_name: String },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ResolverResponse {
    Ok { socket_path: String },
    Error { message: String },
}

// --- MONOREPO REGISTRATION ---

#[derive(Deserialize)]
struct PartialManifest {
    cell: Option<PartialCell>,
    local: Option<std::collections::HashMap<String, String>>,
    workspace: Option<PartialWorkspace>,
}
#[derive(Deserialize)]
struct PartialCell {
    name: String,
}
#[derive(Deserialize)]
struct PartialWorkspace {
    namespace: String,
}

pub fn register() {
    // Only run if we are in a build script context
    if std::env::var("CARGO_MANIFEST_DIR").is_err() {
        return;
    }

    if let Err(e) = register_monorepo() {
        // Don't fail the build, just warn
        println!("cargo:warning=Cell monorepo registration failed: {}", e);
    }
}

fn register_monorepo() -> Result<()> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let start_path = PathBuf::from(manifest_dir);

    // 1. Find Repo Root
    let repo_root = match find_git_root(&start_path) {
        Some(p) => p,
        None => return Ok(()), // Not in a git repo, skip auto-registration
    };

    let home = dirs::home_dir().context("No HOME dir")?;
    let registry_dir = home.join(".cell/registry");
    fs::create_dir_all(&registry_dir)?;

    // 2. Check for Workspace Namespace (in root Cell.toml)
    let mut namespace = None;
    let root_manifest = repo_root.join("Cell.toml");
    if root_manifest.exists() {
        if let Ok(content) = fs::read_to_string(&root_manifest) {
            if let Ok(m) = toml::from_str::<PartialManifest>(&content) {
                if let Some(ws) = m.workspace {
                    namespace = Some(ws.namespace);
                }
            }
        }
    }

    // 3. Walk Repo for Cell.toml files
    for entry in WalkDir::new(&repo_root).into_iter().filter_map(|e| e.ok()) {
        if entry.file_name() == "Cell.toml" {
            process_cell_toml(
                entry.path(),
                &repo_root,
                &registry_dir,
                namespace.as_deref(),
            )?;
        }
    }

    Ok(())
}

fn process_cell_toml(
    path: &Path,
    _repo_root: &Path,
    registry_dir: &Path,
    namespace: Option<&str>,
) -> Result<()> {
    let content = fs::read_to_string(path)?;
    let manifest: PartialManifest = toml::from_str(&content)?;
    let dir = path.parent().unwrap();

    // A. Self-Registration
    if let Some(cell) = manifest.cell {
        let name = if let Some(ns) = namespace {
            format!("{}-{}", ns, cell.name)
        } else {
            cell.name
        };
        create_symlink(registry_dir, &name, dir)?;
    }

    // B. Local Dependency Registration
    if let Some(locals) = manifest.local {
        for (alias, rel_path) in locals {
            let target_path = dir.join(rel_path);
            if let Ok(abs_path) = fs::canonicalize(&target_path) {
                create_symlink(registry_dir, &alias, &abs_path)?;
            } else {
                println!(
                    "cargo:warning=Could not resolve local dependency '{}' at {:?}",
                    alias, target_path
                );
            }
        }
    }

    Ok(())
}

fn create_symlink(registry: &Path, name: &str, target: &Path) -> Result<()> {
    let link = registry.join(name);

    // Idempotency: remove existing if it exists
    if link.exists() || link.is_symlink() {
        fs::remove_file(&link).ok();
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link)?;

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, &link)?;

    Ok(())
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(p) => current = p,
            None => return None,
        }
    }
}

// --- RESOLVER LOGIC ---

pub fn resolve(cell_name: &str) -> Result<String> {
    // 1. CIRCULAR DEPENDENCY BREAKER
    // Kernel cells (and the CLI tool) know where system services live deterministically.
    let current_pkg = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    if is_kernel_cell(&current_pkg) {
        let home = dirs::home_dir().expect("No HOME directory");
        let path = home
            .join(".cell/runtime/system")
            .join(format!("{}.sock", cell_name));
        return Ok(path.to_string_lossy().to_string());
    }

    // 2. Normal Discovery
    let home = dirs::home_dir().expect("No HOME directory");
    let runtime_dir = home.join(".cell/runtime/system");
    let mycelium_sock = runtime_dir.join("mycelium.sock");

    // Connect or Bootstrap
    let mut stream = match UnixStream::connect(&mycelium_sock) {
        Ok(s) => s,
        Err(_) => bootstrap_mycelium(&mycelium_sock)?,
    };

    // Send Request
    let req = ResolverRequest::EnsureRunning {
        cell_name: cell_name.to_string(),
    };
    let req_json = serde_json::to_vec(&req)?;

    let len = req_json.len() as u32;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&req_json)?;

    // Read Response
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut resp_buf = vec![0u8; len];
    stream.read_exact(&mut resp_buf)?;

    let resp: ResolverResponse = serde_json::from_slice(&resp_buf)?;

    match resp {
        ResolverResponse::Ok { socket_path } => Ok(socket_path),
        ResolverResponse::Error { message } => bail!("Resolution failed: {}", message),
    }
}

fn is_kernel_cell(pkg_name: &str) -> bool {
    matches!(
        pkg_name,
        "mycelium"
            | "hypervisor"
            | "builder"
            | "nucleus"
            | "axon"
            | "cell-cli"
            | "mesh"
            | "observer"
    )
}

fn bootstrap_mycelium(socket_path: &Path) -> Result<UnixStream> {
    eprintln!(
        "warning: [cell-build] Mycelium not found at {:?}. Bootstrapping mesh...",
        socket_path
    );

    let infra_target_dir = std::env::temp_dir().join("cell-infra-build");
    std::fs::create_dir_all(&infra_target_dir).ok();

    // Spawn mycelium, STRIPPING environment variables that might cause it to run in test mode
    let status = Command::new("cargo")
        .args(&["run", "--release", "-p", "mycelium"])
        .env("CELL_DAEMON", "1")
        .env("CARGO_TARGET_DIR", infra_target_dir)
        // CRITICAL: Prevent inheriting test environment paths
        .env_remove("CELL_SOCKET_DIR")
        .env_remove("CELL_NODE_ID")
        .env_remove("CELL_ORGANISM")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    if let Err(e) = status {
        bail!("Failed to spawn Mycelium: {}", e);
    }

    // Wait for socket
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        if socket_path.exists() {
            if let Ok(stream) = UnixStream::connect(socket_path) {
                return Ok(stream);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    bail!("Timed out waiting for Mycelium to boot.");
}

pub struct CellBuilder;

impl CellBuilder {
    pub fn configure() -> Self {
        Self
    }

    pub fn extract_macros(self) -> Result<Self> {
        Ok(self)
    }
}

pub fn load_and_flatten_source(entry_path: &Path) -> Result<syn::File> {
    let content = fs::read_to_string(entry_path)
        .with_context(|| format!("Failed to read DNA entry file: {:?}", entry_path))?;
    let mut file = parse_file(&content)?;

    let base_dir = entry_path.parent().unwrap_or_else(|| Path::new("."));
    let mut flattener = ModuleFlattener {
        base_dir: base_dir.to_path_buf(),
        errors: Vec::new(),
    };
    flattener.visit_file_mut(&mut file);
    if let Some(err) = flattener.errors.first() {
        bail!("Module resolution failed: {}", err);
    }
    Ok(file)
}

struct ModuleFlattener {
    base_dir: PathBuf,
    errors: Vec<String>,
}

impl VisitMut for ModuleFlattener {
    fn visit_item_mod_mut(&mut self, node: &mut syn::ItemMod) {
        if node.content.is_none() {
            let mod_name = node.ident.to_string();
            let file_path = self.base_dir.join(format!("{}.rs", mod_name));
            let target_path = if file_path.exists() {
                Some(file_path)
            } else {
                let mod_path = self.base_dir.join(&mod_name).join("mod.rs");
                if mod_path.exists() {
                    Some(mod_path)
                } else {
                    None
                }
            };

            if let Some(path) = target_path {
                match fs::read_to_string(&path) {
                    Ok(content) => match parse_file(&content) {
                        Ok(mut file) => {
                            let sub_base_dir =
                                if path.file_name().and_then(|n| n.to_str()) == Some("mod.rs") {
                                    path.parent().unwrap().to_path_buf()
                                } else {
                                    path.parent().unwrap().join(&mod_name)
                                };
                            let mut sub_visitor = ModuleFlattener {
                                base_dir: sub_base_dir,
                                errors: Vec::new(),
                            };
                            sub_visitor.visit_file_mut(&mut file);
                            if !sub_visitor.errors.is_empty() {
                                self.errors.extend(sub_visitor.errors);
                            }
                            node.content = Some((syn::token::Brace::default(), file.items));
                        }
                        Err(e) => self
                            .errors
                            .push(format!("Failed to parse {:?}: {}", path, e)),
                    },
                    Err(e) => self
                        .errors
                        .push(format!("Failed to read {:?}: {}", path, e)),
                }
            } else {
                self.errors.push(format!(
                    "Module '{}' not found in {:?}",
                    mod_name, self.base_dir
                ));
            }
        } else {
            let old_base = self.base_dir.clone();
            self.base_dir = self.base_dir.join(node.ident.to_string());
            syn::visit_mut::visit_item_mod_mut(self, node);
            self.base_dir = old_base;
        }
    }
}
