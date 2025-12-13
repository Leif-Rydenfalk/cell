// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::path::{Path, PathBuf};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::Duration;
use anyhow::{Result, Context, bail, anyhow};
use serde::{Deserialize, Serialize};
use syn::{parse_file, Item, Type, FnArg, Pat, ReturnType, File, Attribute};
use syn::visit_mut::VisitMut;
use quote::{quote, format_ident, ToTokens};
use convert_case::{Case, Casing};

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

// --- RESOLVER LOGIC ---

pub fn resolve(cell_name: &str) -> Result<String> {
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

fn bootstrap_mycelium(socket_path: &Path) -> Result<UnixStream> {
    // We print to stderr so it shows up in cargo build output
    eprintln!("warning: [cell-build] Mycelium not found at {:?}. Bootstrapping mesh...", socket_path);

    // FIX: Use a separate target directory for infrastructure to avoid deadlocking 
    // against the main 'cargo run' process holding the lock on ./target
    let infra_target_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target")
        .join("infra");

    std::fs::create_dir_all(&infra_target_dir).ok();

    // Try to spawn mycelium via cargo. 
    let status = Command::new("cargo")
        .args(&["run", "--release", "-p", "mycelium"])
        .env("CELL_DAEMON", "1")
        .env("CARGO_TARGET_DIR", infra_target_dir) // <--- PREVENTS DEADLOCK
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn();

    if let Err(e) = status {
        bail!("Failed to spawn Mycelium: {}", e);
    }

    // Wait for socket
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
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

// --- EXISTING BUILDER LOGIC ---

pub fn register() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let pkg_name = std::env::var("CARGO_PKG_NAME").unwrap();
    let home = dirs::home_dir().expect("No HOME directory");
    let registry = home.join(".cell/registry");
    
    if let Err(e) = fs::create_dir_all(&registry) {
        println!("cargo:warning=Failed to create cell registry: {}", e);
        return;
    }
    
    let link_path = registry.join(&pkg_name);
    let target_path = Path::new(&manifest_dir);

    if link_path.exists() {
        if let Ok(existing) = fs::read_link(&link_path) {
            if existing == target_path { return; }
        }
        let _ = fs::remove_file(&link_path);
    }

    #[cfg(unix)]
    {
        if let Err(e) = std::os::unix::fs::symlink(target_path, &link_path) {
            println!("cargo:warning=Failed to register cell '{}': {}", pkg_name, e);
        } else {
            println!("cargo:warning=Registered cell '{}' in ~/.cell/registry", pkg_name);
        }
    }
}

pub struct CellBuilder {
    cell_name: String,
    source_path: PathBuf,
}

impl CellBuilder {
    pub fn configure() -> Self {
        let cell_name = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "unknown".to_string());
        let source_path = PathBuf::from(".");
        Self { cell_name, source_path }
    }

    pub fn extract_macros(self) -> Result<Self> {
        Ok(self) // Simplified for this context
    }
}

pub fn load_and_flatten_source(entry_path: &Path) -> Result<File> {
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
                if mod_path.exists() { Some(mod_path) } else { None }
            };

            if let Some(path) = target_path {
                match fs::read_to_string(&path) {
                    Ok(content) => {
                        match parse_file(&content) {
                            Ok(mut file) => {
                                let sub_base_dir = if path.file_name().and_then(|n| n.to_str()) == Some("mod.rs") {
                                    path.parent().unwrap().to_path_buf()
                                } else {
                                    path.parent().unwrap().join(&mod_name)
                                };
                                let mut sub_visitor = ModuleFlattener { base_dir: sub_base_dir, errors: Vec::new() };
                                sub_visitor.visit_file_mut(&mut file);
                                if !sub_visitor.errors.is_empty() { self.errors.extend(sub_visitor.errors); }
                                node.content = Some((syn::token::Brace::default(), file.items));
                            },
                            Err(e) => self.errors.push(format!("Failed to parse {:?}: {}", path, e)),
                        }
                    },
                    Err(e) => self.errors.push(format!("Failed to read {:?}: {}", path, e)),
                }
            } else {
                self.errors.push(format!("Module '{}' not found in {:?}", mod_name, self.base_dir));
            }
        } else {
            let old_base = self.base_dir.clone();
            self.base_dir = self.base_dir.join(node.ident.to_string());
            syn::visit_mut::visit_item_mod_mut(self, node);
            self.base_dir = old_base;
        }
    }
}