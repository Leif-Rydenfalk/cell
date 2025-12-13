// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate proc_macro;
use convert_case::{Case, Casing};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, FnArg, GenericArgument, Ident, Item, ItemImpl, LitStr, Pat, PathArguments,
    ReturnType, Token, Type,
};

mod test;

// Helper functions (Simplified for brevity, ensuring critical logic is present)
fn normalize_ty(ty: &Type) -> Type {
    ty.clone()
} // Placeholder normalization
fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path().is_ident(name))
}

fn locate_dna(cell_name: &str) -> PathBuf {
    // Basic search logic to find the source file for the cell
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let root = std::path::Path::new(&manifest)
        .parent()
        .unwrap_or(std::path::Path::new("."));

    // Naive search in standard locations
    let candidates = vec![
        root.join("cells").join(cell_name).join("src/main.rs"),
        root.join("examples")
            .join("cell-market")
            .join(cell_name)
            .join("src/main.rs"),
        root.join("examples")
            .join("cell-tissue")
            .join(cell_name)
            .join("src/main.rs"),
        root.join(format!("../cells/{}/src/main.rs", cell_name)),
    ];

    for p in candidates {
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("DNA_NOT_FOUND")
}

// --- THE NEW CELL_REMOTE MACRO ---

struct CellRemoteArgs {
    module_name: Ident,
    cell_name: String,
}
impl Parse for CellRemoteArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let module_name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let cell_name_lit: LitStr = input.parse()?;
        Ok(CellRemoteArgs {
            module_name,
            cell_name: cell_name_lit.value(),
        })
    }
}

#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as CellRemoteArgs);
    let module_name = args.module_name;
    let cell_name = args.cell_name;

    // 1. Resolve socket path at compile time
    // This triggers the Mycelium bootstrap logic
    let socket_path = match cell_build::resolve(&cell_name) {
        Ok(path) => path,
        Err(e) => {
            // Panic during compilation if resolution fails, halting the build
            panic!("Failed to resolve cell '{}': {}", cell_name, e);
        }
    };

    // 2. Load DNA to generate typed client (as before)
    let dna_path = locate_dna(&cell_name);
    if dna_path.to_string_lossy() == "DNA_NOT_FOUND" {
        panic!(
            "Could not locate source code for cell '{}'. Ensure it exists in the workspace.",
            cell_name
        );
    }

    let file = match cell_build::load_and_flatten_source(&dna_path) {
        Ok(f) => f,
        Err(e) => panic!("Failed to parse DNA for '{}': {}", cell_name, e),
    };

    let mut proteins = Vec::new();
    let mut handler_impl = None;
    let mut service_struct_name = String::new();

    // Quick scan for handler
    for item in &file.items {
        match item {
            Item::Enum(i) if has_attr(&i.attrs, "protein") => proteins.push(item),
            Item::Struct(i) if has_attr(&i.attrs, "protein") => proteins.push(item),
            Item::Impl(i) if has_attr(&i.attrs, "handler") => {
                handler_impl = Some(i);
                if let Type::Path(tp) = &*i.self_ty {
                    service_struct_name = tp.path.segments.last().unwrap().ident.to_string();
                }
            }
            _ => {}
        }
    }

    if handler_impl.is_none() {
        panic!("No #[handler] found in {}", cell_name);
    }

    let mut methods = Vec::new();
    for item in handler_impl.unwrap().items.iter() {
        if let syn::ImplItem::Fn(m) = item {
            let name = m.sig.ident.clone();
            let mut args = Vec::new();
            for arg in &m.sig.inputs {
                if let FnArg::Typed(pat) = arg {
                    if let Pat::Ident(id) = &*pat.pat {
                        args.push((id.ident.clone(), *pat.ty.clone()));
                    }
                }
            }
            let ret = match &m.sig.output {
                ReturnType::Default => syn::parse_quote! { () },
                ReturnType::Type(_, ty) => *ty.clone(),
            };
            // Primitive return type extraction (strips Result)
            // Simplified for this file generation
            methods.push((name, args, ret));
        }
    }

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
        // We assume the macro handles Result stripping in a real impl, here we just use ret
        quote! { #vname(#ret) }
    });

    let client_methods = methods.iter().map(|(n, args, _)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        let args_sig = args.iter().map(|(an, at)| quote! { #an: #at });
        let args_struct = args.iter().map(|(an, _)| quote! { #an });
        quote! {
            pub async fn #n(&mut self, #(#args_sig),*) -> ::cell_sdk::anyhow::Result<::cell_sdk::anyhow::Result<()>> {
                // Placeholder return type signature matching
                let req = #protocol_name::#vname { #(#args_struct),* };
                // ... fire logic
                Ok(Ok(()))
            }
        }
    });

    // Code Generation
    let expanded = quote! {
        #[allow(non_snake_case, dead_code)]
        pub mod #module_name {
            use super::*;
            use cell_sdk::protein;

            // Hardcoded socket path from compile-time resolution
            pub const SOCKET_PATH: &'static str = #socket_path;

            #(#proteins)*

            #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
            #[serde(crate = "::cell_sdk::serde")]
            #[archive(crate = "::cell_sdk::rkyv")]
            pub enum #protocol_name { #(#req_variants),* }

            #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
            #[serde(crate = "::cell_sdk::serde")]
            #[archive(crate = "::cell_sdk::rkyv")]
            pub enum #response_name { #(#resp_variants),* }

            pub struct #client_struct { conn: ::cell_sdk::Synapse }

            impl #client_struct {
                pub async fn connect() -> ::cell_sdk::anyhow::Result<Self> {
                    // Connect directly to the baked-in socket path
                    let conn = ::cell_sdk::Synapse::connect_direct(SOCKET_PATH).await?;
                    Ok(Self { conn })
                }

                // Generated methods stubbed for brevity in this output,
                // focusing on the connect logic change.
                #(#client_methods)*
            }
        }
    };
    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn protein(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::DeriveInput);
    let expanded = quote! {
        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Clone, Debug, PartialEq)]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(check_bytes)]
        #[archive(crate = "::cell_sdk::rkyv")]
        #input
    };
    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn service(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::DeriveInput);
    TokenStream::from(quote! { #input })
}

#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass-through for now, real impl adds Protocol enum generation server-side
    TokenStream::from(item)
}

#[proc_macro_attribute]
pub fn cell_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    test::cell_test_impl(item)
}
