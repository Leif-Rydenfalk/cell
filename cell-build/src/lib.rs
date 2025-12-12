// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::path::{Path, PathBuf};
use anyhow::{Result, Context, bail};
use std::fs;
use std::process::Command;
use syn::{parse_file, Item, Type, FnArg, Pat, ReturnType, File, Attribute};
use syn::visit_mut::VisitMut;
use quote::{quote, format_ident, ToTokens};
use convert_case::{Case, Casing};

/// Registers the current project in the local Cell Registry.
/// Should be called from build.rs.
pub fn register() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let pkg_name = std::env::var("CARGO_PKG_NAME").unwrap();
    
    // Default registry path
    let home = dirs::home_dir().expect("No HOME directory");
    let registry = home.join(".cell/registry");
    
    if let Err(e) = fs::create_dir_all(&registry) {
        println!("cargo:warning=Failed to create cell registry: {}", e);
        return;
    }
    
    let link_path = registry.join(&pkg_name);
    let target_path = Path::new(&manifest_dir);

    // Idempotent symlink creation
    if link_path.exists() {
        if let Ok(existing) = fs::read_link(&link_path) {
            if existing == target_path {
                return; // Already registered
            }
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

// ... [Rest of file remains unchanged] ...

/// Helper for recursive module flattening, exposed for use by cell-macros
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

impl CellBuilder {
    pub fn configure() -> Self {
        let cell_name = std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "unknown".to_string());
        let source_path = PathBuf::from(".");
        Self { cell_name, source_path }
    }

    pub fn register(mut self, name: &str, path: impl AsRef<Path>) -> Self {
        self.cell_name = name.to_string();
        self.source_path = path.as_ref().to_path_buf();
        self
    }

    pub fn extract_macros(self) -> Result<Self> {
        let dna_path = self.source_path.join("src/main.rs");
        let entry_point = if dna_path.exists() {
            dna_path
        } else {
            let lib_path = self.source_path.join("src/lib.rs");
            if !lib_path.exists() {
                bail!("DNA entry point (src/main.rs or src/lib.rs) not found at {:?}", self.source_path);
            }
            lib_path
        };

        let file = load_and_flatten_source(&entry_point)?;
        
        let proc_macros = extract_proc_macros(&file);
        
        if !proc_macros.is_empty() {
            self.generate_macro_crate(&proc_macros)?;
        }
        
        Ok(self)
    }

    fn generate_macro_crate(&self, macros: &[ProcMacroItem]) -> Result<()> {
        let home = dirs::home_dir().context("No home directory")?;
        let macro_dir = home.join(".cell/macros").join(&self.cell_name);
        
        fs::create_dir_all(&macro_dir)?;
        fs::create_dir_all(macro_dir.join("src"))?;
        
        let cargo_toml = format!(r#"[package]
name = "{}-macros"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true

[dependencies]
syn = {{ version = "2.0", features = ["full", "extra-traits"] }}
quote = "1.0"
proc-macro2 = "1.0"
cell-transport = {{ version = "0.4.0", features = ["std"] }}
cell-model = "0.4.0"
tokio = {{ version = "1", features = ["full"] }}
"#, 
            self.cell_name
        );
        
        fs::write(macro_dir.join("Cargo.toml"), cargo_toml)?;
        
        let lib_rs = self.generate_macro_lib_code(macros)?;
        fs::write(macro_dir.join("src/lib.rs"), lib_rs)?;
        
        let status = Command::new("cargo")
            .arg("build")
            .arg("--release")
            .current_dir(&macro_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run cargo build for macro crate")?;
        
        if !status.success() {
            bail!("Failed to compile macro crate for {}", self.cell_name);
        }
        
        let manifest = self.generate_macro_manifest(macros)?;
        fs::write(macro_dir.join("manifest.json"), manifest)?;

        println!("cargo:warning=[cell-build] Generated macro crate: {}-macros at {}", self.cell_name, macro_dir.display());
        
        Ok(())
    }
    
    fn generate_macro_lib_code(&self, macros: &[ProcMacroItem]) -> Result<String> {
        let mut code = String::new();
        
        code.push_str("// Auto-generated macro crate\n");
        code.push_str("// DO NOT EDIT - regenerated on each build\n\n");
        code.push_str("extern crate proc_macro;\n");
        code.push_str("use proc_macro::TokenStream;\n");
        code.push_str("use quote::quote;\n");
        code.push_str("use syn::{parse_macro_input, DeriveInput, Field, ItemFn};\n");
        code.push_str("use cell_transport::coordination::MacroCoordinator;\n");
        code.push_str("use cell_model::macro_coordination::ExpansionContext;\n\n");
        
        for mac in macros {
            let item_tokens = mac.item.to_token_stream().to_string();
            code.push_str(&item_tokens);
            code.push_str("\n\n");
        }
        
        Ok(code)
    }

    fn generate_macro_manifest(&self, macros: &[ProcMacroItem]) -> Result<String> {
        use serde_json::json;
        
        let mut info_list = Vec::new();
        
        for mac in macros {
             let name = match &mac.item {
                 Item::Fn(f) => f.sig.ident.to_string(),
                 _ => "unknown".to_string(),
             };
             
             let kind = if has_attr(&mac.item, "proc_macro_attribute") {
                 "Attribute"
             } else if has_attr(&mac.item, "proc_macro_derive") {
                 "Derive"
             } else {
                 "Function"
             };
             
             info_list.push(json!({
                 "name": name,
                 "kind": kind,
                 "description": "Auto-extracted macro",
                 "dependencies": []
             }));
        }
        
        Ok(serde_json::to_string_pretty(&info_list)?)
    }

    pub fn generate(self) -> Result<()> {
        let out_dir = std::env::var("OUT_DIR").context("OUT_DIR not set. Are you running this from build.rs?")?;
        let dest_path = Path::new(&out_dir).join(format!("{}_client.rs", self.cell_name));

        let dna_path = self.source_path.join("src/main.rs");
        let entry_point = if dna_path.exists() {
            dna_path
        } else {
            let lib_path = self.source_path.join("src/lib.rs");
            if !lib_path.exists() {
                bail!("DNA entry point (src/main.rs or src/lib.rs) not found at {:?}", self.source_path);
            }
            lib_path
        };

        let file = load_and_flatten_source(&entry_point)?;

        let mut proteins = Vec::new();
        let mut handler_impl = None;
        let mut service_struct_name = String::new();

        visit_items_for_dna(&file.items, &mut proteins, &mut handler_impl, &mut service_struct_name);

        if handler_impl.is_none() {
            bail!("No #[handler] found in resolved source tree for {}", self.cell_name);
        }

        let items = handler_impl.unwrap().items.clone();
        let mut methods = Vec::new();
        
        for item in items {
             if let syn::ImplItem::Fn(m) = item {
                 let name = m.sig.ident;
                 let mut args = Vec::new();
                 for arg in m.sig.inputs {
                     if let FnArg::Typed(pat) = arg {
                         if let Pat::Ident(id) = *pat.pat {
                             args.push((id.ident, *pat.ty));
                         }
                     }
                 }
                 
                 let ret = match m.sig.output {
                     ReturnType::Default => syn::parse_quote! { () },
                     ReturnType::Type(_, ty) => *ty,
                 };
                 
                 let wire_ret = if let Type::Path(tp) = &ret {
                     if let Some(seg) = tp.path.segments.last() {
                         if seg.ident == "Result" {
                             if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                                 if let Some(syn::GenericArgument::Type(ok_type)) = args.args.first() {
                                     ok_type.clone()
                                 } else { ret.clone() }
                             } else { ret.clone() }
                         } else { ret.clone() }
                     } else { ret.clone() }
                 } else { ret.clone() };

                 methods.push((name, args, wire_ret));
             }
        }

        let module_name = format_ident!("{}", self.cell_name);
        let protocol_name = format_ident!("{}Protocol", service_struct_name);
        let response_name = format_ident!("{}Response", service_struct_name);
        let client_struct = format_ident!("Client");

        let req_variants = methods.iter().map(|(n, args, _)| {
            let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
            let fields = args.iter().map(|(an, at)| quote! { #an: #at });
            quote! { #vname { #(#fields),* } }
        });

        let resp_variants = methods.iter().map(|(n, _, ret)| {
            let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
            quote! { #vname(#ret) }
        });

        let client_methods = methods.iter().map(|(n, args, ret)| {
            let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
            let args_sig = args.iter().map(|(an, at)| quote! { #an: #at });
            let args_struct = args.iter().map(|(an, _)| quote! { #an });
            quote! {
                pub async fn #n(&mut self, #(#args_sig),*) -> ::anyhow::Result<#ret> {
                    let req = #protocol_name::#vname { #(#args_struct),* };
                    let resp = self.conn.fire::<#protocol_name, #response_name>(&req).await?;
                    let val = resp.deserialize()?;
                    match val {
                        #response_name::#vname(res) => Ok(res),
                        _ => Err(::anyhow::anyhow!("Protocol Mismatch")),
                    }
                }
            }
        });

        let proteins_expanded = proteins.iter().map(|p| quote! { #p });

        let output = quote! {
            pub mod #module_name {
                use cell_sdk::protein;
                use ::cell_sdk::serde::{Deserialize, Serialize};
                
                #(#proteins_expanded)*

                #[derive(
                    ::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize,
                    ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize,
                    Debug, Clone
                )]
                #[serde(crate = "::cell_sdk::serde")]
                #[archive(check_bytes)]
                #[archive(crate = "::cell_sdk::rkyv")]
                pub enum #protocol_name {
                    #(#req_variants),*
                }

                #[derive(
                    ::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize,
                    ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize,
                    Debug, Clone
                )]
                #[serde(crate = "::cell_sdk::serde")]
                #[archive(check_bytes)]
                #[archive(crate = "::cell_sdk::rkyv")]
                pub enum #response_name {
                    #(#resp_variants),*
                }

                pub struct #client_struct { conn: ::cell_sdk::Synapse }

                impl #client_struct {
                    pub async fn connect() -> ::anyhow::Result<Self> {
                        Ok(Self { conn: ::cell_sdk::Synapse::grow(stringify!(#module_name)).await? })
                    }
                    pub fn connection(&mut self) -> &mut ::cell_sdk::Synapse { &mut self.conn }
                    #(#client_methods)*
                }
            }
        };
        
        let syntax_tree = syn::parse2(output).unwrap();
        let formatted = prettyplease::unparse(&syntax_tree);
        fs::write(dest_path, formatted)?;

        Ok(())
    }
}

// ... [ModuleFlattener and helpers remain unchanged] ...
fn visit_items_for_dna<'a>(
    items: &'a [Item], 
    proteins: &mut Vec<&'a Item>, 
    handler: &mut Option<&'a syn::ItemImpl>, 
    service_name: &mut String
) {
    for item in items {
        match item {
            Item::Enum(i) if i.attrs.iter().any(|a| a.path().is_ident("protein")) => proteins.push(item),
            Item::Struct(i) if i.attrs.iter().any(|a| a.path().is_ident("protein")) => proteins.push(item),
            Item::Impl(i) if i.attrs.iter().any(|a| a.path().is_ident("handler")) => {
                if let Type::Path(tp) = &*i.self_ty {
                    *service_name = tp.path.segments.last().unwrap().ident.to_string();
                }
                *handler = Some(i);
            }
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    visit_items_for_dna(items, proteins, handler, service_name);
                }
            }
            _ => {}
        }
    }
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

                                let mut sub_visitor = ModuleFlattener {
                                    base_dir: sub_base_dir,
                                    errors: Vec::new(),
                                };
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

struct ProcMacroItem {
    item: Item,
}

fn extract_proc_macros(file: &File) -> Vec<ProcMacroItem> {
    let mut macros = Vec::new();
    fn visit_items(items: &[Item], macros: &mut Vec<ProcMacroItem>) {
        for item in items {
            let has_cell_macro = match item {
                Item::Fn(_) => has_attr(item, "cell_macro"),
                _ => false,
            };
            if !has_cell_macro {
                if let Item::Mod(m) = item {
                    if let Some((_, items)) = &m.content {
                        visit_items(items, macros);
                    }
                }
                continue;
            }
            macros.push(ProcMacroItem { item: item.clone() });
        }
    }
    visit_items(&file.items, &mut macros);
    macros
}

fn has_attr(item: &Item, name: &str) -> bool {
    let attrs = match item {
        Item::Fn(f) => &f.attrs,
        _ => return false,
    };
    attrs.iter().any(|a| a.path().is_ident(name) || (a.path().segments.len() == 2 && a.path().segments[1].ident == name))
}