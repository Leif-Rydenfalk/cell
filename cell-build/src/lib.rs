// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{anyhow, bail, Context, Result};
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

// --- MACRO RUNNER (Optimized with Binary Caching) ---

pub struct MacroRunner;

impl MacroRunner {
    pub fn run(layer: &str, feature: &str, struct_source: &str) -> Result<String> {
        let cell_name = layer;
        let home = dirs::home_dir().context("No HOME")?;
        let registry_dir = home.join(".cell/registry");
        let cell_path = registry_dir.join(cell_name);

        if !cell_path.exists() {
            bail!("Macro provider cell '{}' not found in registry.", cell_name);
        }

        // 1. Read Cell.toml to find function name
        let manifest_path = cell_path.join("Cell.toml");
        let manifest_content = fs::read_to_string(&manifest_path)?;

        #[derive(Deserialize)]
        struct MacroManifest {
            #[serde(default)]
            macros: std::collections::HashMap<String, String>,
        }
        let m: MacroManifest = toml::from_str(&manifest_content)?;
        let fn_name = m.macros.get(feature).ok_or_else(|| {
            anyhow!(
                "Cell '{}' does not export macro feature '{}'",
                cell_name,
                feature
            )
        })?;

        // 2. Hash the provider source to see if we can use a cached binary
        let provider_hash = Self::compute_hash(&cell_path)?;
        let cache_dir = home
            .join(".cell/cache/macros")
            .join(cell_name)
            .join(feature);
        let bin_path = cache_dir.join("runner");
        let hash_path = cache_dir.join("source.hash");

        fs::create_dir_all(&cache_dir)?;

        let is_cached = bin_path.exists()
            && fs::read_to_string(&hash_path)
                .map(|h| h == provider_hash)
                .unwrap_or(false);

        if !is_cached {
            Self::compile_runner(
                cell_name,
                &cell_path,
                fn_name,
                &bin_path,
                &provider_hash,
                &hash_path,
            )?;
        }

        // 3. Execute the cached/compiled binary
        let mut child = Command::new(&bin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(struct_source.as_bytes())?;
        drop(stdin);

        let output = child.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Macro expansion failed:\n{}", stderr);
        }

        Ok(String::from_utf8(output.stdout)?)
    }

    fn compile_runner(
        cell: &str,
        cell_path: &Path,
        fn_name: &str,
        bin_dest: &Path,
        hash: &str,
        hash_dest: &Path,
    ) -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("compile_cell_macro_{}", cell));
        fs::create_dir_all(&temp_dir)?;

        let cargo_toml = format!(
            r#"[package]
name = "macro_runner"
version = "0.0.0"
edition = "2021"
[dependencies]
{} = {{ path = {:?} }}
syn = {{ version = "2.0", features = ["full"] }}
quote = "1.0"
"#,
            cell, cell_path
        );

        let main_rs = format!(
            r#"
use {}::{};
use std::io::Read;
fn main() {{
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let ast: syn::ItemStruct = syn::parse_str(&input).expect("Parse failed");
    let tokens = {}(&ast);
    println!("{{}}", tokens);
}}
"#,
            cell, fn_name, fn_name
        );

        fs::write(temp_dir.join("Cargo.toml"), cargo_toml)?;
        fs::create_dir_all(temp_dir.join("src"))?;
        fs::write(temp_dir.join("src/main.rs"), main_rs)?;

        let status = Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(&temp_dir)
            .status()?;

        if !status.success() {
            bail!("Failed to compile macro runner for {}", cell);
        }

        let built_bin = temp_dir.join("target/release/macro_runner");
        fs::copy(&built_bin, bin_dest)?;
        fs::write(hash_dest, hash)?;

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    fn compute_hash(path: &Path) -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
            if entry
                .path()
                .extension()
                .map_or(false, |ext| ext == "rs" || ext == "toml")
            {
                hasher.update(&fs::read(entry.path())?);
            }
        }
        Ok(hasher.finalize().to_hex().to_string())
    }
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
    if std::env::var("CARGO_MANIFEST_DIR").is_err() {
        return;
    }
    if let Err(e) = register_monorepo() {
        println!("cargo:warning=Cell monorepo registration failed: {}", e);
    }
}

fn register_monorepo() -> Result<()> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let start_path = PathBuf::from(manifest_dir);
    let repo_root = match find_git_root(&start_path) {
        Some(p) => p,
        None => return Ok(()),
    };

    let home = dirs::home_dir().context("No HOME dir")?;
    let registry_dir = home.join(".cell/registry");
    fs::create_dir_all(&registry_dir)?;

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

    if let Some(cell) = manifest.cell {
        let name = if let Some(ns) = namespace {
            format!("{}-{}", ns, cell.name)
        } else {
            cell.name
        };
        create_symlink(registry_dir, &name, dir)?;
    }

    if let Some(locals) = manifest.local {
        for (alias, rel_path) in locals {
            let target_path = dir.join(rel_path);
            if let Ok(abs_path) = fs::canonicalize(&target_path) {
                create_symlink(registry_dir, &alias, &abs_path)?;
            }
        }
    }
    Ok(())
}

fn create_symlink(registry: &Path, name: &str, target: &Path) -> Result<()> {
    let link = registry.join(name);
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
    let current_pkg = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    if is_kernel_cell(&current_pkg) {
        let home = dirs::home_dir().expect("No HOME directory");
        let path = home
            .join(".cell/runtime/system")
            .join(format!("{}.sock", cell_name));
        return Ok(path.to_string_lossy().to_string());
    }

    let home = dirs::home_dir().expect("No HOME directory");
    let runtime_dir = home.join(".cell/runtime/system");
    let mycelium_sock = runtime_dir.join("mycelium.sock");

    let mut stream = match UnixStream::connect(&mycelium_sock) {
        Ok(s) => s,
        Err(_) => bootstrap_mycelium(&mycelium_sock)?,
    };

    let req = ResolverRequest::EnsureRunning {
        cell_name: cell_name.to_string(),
    };
    let req_json = serde_json::to_vec(&req)?;
    let len = req_json.len() as u32;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&req_json)?;

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
        "mycelium" | "hypervisor" | "builder" | "axon" | "cell-cli" | "mesh" | "observer"
    )
}

fn bootstrap_mycelium(socket_path: &Path) -> Result<UnixStream> {
    let infra_target_dir = std::env::temp_dir().join("cell-infra-build");
    std::fs::create_dir_all(&infra_target_dir).ok();

    let _ = Command::new("cargo")
        .args(&["run", "--release", "-p", "mycelium"])
        .env("CARGO_TARGET_DIR", infra_target_dir)
        .env_remove("CELL_SOCKET_DIR")
        .env_remove("CELL_NODE_ID")
        .env_remove("CELL_ORGANISM")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

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
    let content = fs::read_to_string(entry_path)?;
    let mut file = parse_file(&content)?;
    let base_dir = entry_path.parent().unwrap_or_else(|| Path::new("."));
    let mut flattener = ModuleFlattener {
        base_dir: base_dir.to_path_buf(),
        errors: Vec::new(),
    };
    flattener.visit_file_mut(&mut file);
    if let Some(err) = flattener.errors.first() {
        bail!("{}", err);
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
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(mut file) = parse_file(&content) {
                        let sub_base_dir =
                            if path.file_name().and_then(|n| n.to_str()) == Some("mod.rs") {
                                path.parent().unwrap().to_path_buf()
                            } else {
                                path.parent().unwrap().join(&mod_name)
                            };
                        let mut sub_v = ModuleFlattener {
                            base_dir: sub_base_dir,
                            errors: Vec::new(),
                        };
                        sub_v.visit_file_mut(&mut file);
                        self.errors.extend(sub_v.errors);
                        node.content = Some((syn::token::Brace::default(), file.items));
                    }
                }
            }
        } else {
            let old_base = self.base_dir.clone();
            self.base_dir = self.base_dir.join(node.ident.to_string());
            syn::visit_mut::visit_item_mod_mut(self, node);
            self.base_dir = old_base;
        }
    }
}
