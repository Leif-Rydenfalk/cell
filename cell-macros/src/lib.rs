// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::{
    parse_macro_input, DeriveInput, Item, ItemImpl, Type, FnArg, Pat, ReturnType,
    spanned::Spanned, Attribute, GenericArgument, PathArguments
};
use convert_case::{Case, Casing};
use std::path::PathBuf;
use std::fs;

// --- Helper Functions ---

fn normalize_ty(ty: &Type) -> Type {
    if let Type::Reference(type_ref) = ty {
        if let Type::Path(type_path) = &*type_ref.elem {
            if let Some(segment) = type_path.path.segments.last() {
                if segment.ident == "Archived" {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                            return inner_ty.clone();
                        }
                    }
                }
            }
        }
    }
    ty.clone()
}

fn sanitize_return_type(ty: &Type) -> Type {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            if seg.ident == "Result" {
                if let PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(GenericArgument::Type(ok_type)) = args.args.first() {
                        return syn::parse_quote! { ::std::result::Result<#ok_type, ::std::string::String> };
                    }
                }
            }
        }
    }
    ty.clone()
}

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path().is_ident(name) || 
        (a.path().segments.len() == 2 && a.path().segments[1].ident == name))
}

fn locate_dna(cell_name: &str) -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SCHEMA_PATH") {
        let path = PathBuf::from(p).join(format!("{}.rs", cell_name));
        if path.exists() { return path; }
    }
    if let Some(home) = dirs::home_dir() {
        let global = home.join(".cell/schema").join(format!("{}.rs", cell_name));
        if global.exists() { return global; }
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("No MANIFEST_DIR");
    let root = std::path::Path::new(&manifest);
    
    if let Some(parent) = root.parent() {
        let sibling = parent.join(cell_name).join("src/main.rs");
        if sibling.exists() { return sibling; }
        let deep_sibling = parent.join("cells").join(cell_name).join("src/main.rs");
        if deep_sibling.exists() { return deep_sibling; }
    }
    panic!("Could not locate DNA for '{}'", cell_name);
}

// --- Macros ---

#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    let self_ty = &input.self_ty;
    let service_name = match &**self_ty {
        Type::Path(tp) => tp.path.segments.last().unwrap().ident.clone(),
        _ => panic!("Handler must implement a struct"),
    };

    let mut methods = Vec::new();
    for item in &input.items {
        if let syn::ImplItem::Fn(m) = item {
             // ... extract method info (args, ret) ...
             // Simplified extraction for brevity of "whole file" context
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
             let wire_ret = sanitize_return_type(&ret);
             methods.push((name, args, ret, wire_ret));
        }
    }

    let protocol_name = format_ident!("{}Protocol", service_name);
    let response_name = format_ident!("{}Response", service_name);

    let req_variants = methods.iter().map(|(n, args, _, _)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        // Needs normalized types for serialization
        let fields = args.iter().map(|(an, at)| {
             let norm = normalize_ty(at);
             quote! { #an: #norm }
        });
        quote! { #vname { #(#fields),* } }
    });

    let resp_variants = methods.iter().map(|(n, _, _, wret)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        quote! { #vname(#wret) }
    });
    
    // Dispatch logic needs to deserialize properly
    let dispatch_arms = methods.iter().map(|(n, args, _, wret)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        let field_names: Vec<_> = args.iter().map(|(an, _)| an).collect();
        // Here we assume arguments come as CheckedArchived (zero-copy)
        // We need to pass them to handler which expects &Archived<T> or similar
        // For simplicity in this implementation, we just pass fields by name.
        quote! {
            ArchivedProtocol::#vname { #(#field_names),* } => {
                let result = self.#n(#(#field_names),*).await;
                // convert result to wret (sanitize result)
                let wire_result = match result {
                    Ok(val) => Ok(val),
                    Err(e) => Err(format!("{:?}", e)), // naive conversion
                };
                Ok(#response_name::#vname(wire_result))
            }
        }
    });

    let expanded = quote! {
        #input

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

        impl #service_name {
            pub async fn serve(self, name: &str) -> ::anyhow::Result<()> {
                let service = std::sync::Arc::new(self);
                ::cell_sdk::Membrane::bind::<_, #protocol_name, #response_name>(
                    name,
                    move |archived_req| {
                        let svc = service.clone();
                        Box::pin(async move {
                            svc.dispatch(archived_req).await
                        })
                    },
                    None
                ).await
            }

            async fn dispatch(
                &self,
                req: &<#protocol_name as ::cell_sdk::rkyv::Archive>::Archived
            ) -> ::anyhow::Result<#response_name> {
                type ArchivedProtocol = <#protocol_name as ::cell_sdk::rkyv::Archive>::Archived;
                match req {
                    #(#dispatch_arms),*
                }
            }
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();
    let parts: Vec<&str> = input_str.split('=').collect();
    if parts.len() != 2 { panic!("Usage: cell_remote!(Module = \"cell_name\")"); }
    
    let module_name = format_ident!("{}", parts[0].trim());
    let cell_name = parts[1].trim().trim_matches(|c| c == '"' || c == ' ');

    // 1. Check if Build Script generated client exists
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let path = PathBuf::from(out_dir).join(format!("{}_client.rs", cell_name));
        if path.exists() {
            let path_str = path.to_str().unwrap();
            return TokenStream::from(quote! {
                include!(#path_str);
                // Re-export specific module as requested alias
                pub use #cell_name::Client as #module_name;
            });
        }
    }

    // 2. Fallback: Parse Source (Macro Mode)
    let dna_path = locate_dna(cell_name);
    let dna_path_str = dna_path.to_str().expect("Invalid path");
    let content = fs::read_to_string(&dna_path).expect("Failed to read DNA");
    let file = syn::parse_file(&content).expect("Failed to parse DNA");

    let mut proteins = Vec::new();
    let mut handler_impl = None;
    let mut service_struct_name = String::new();

    for item in file.items {
        match &item {
            Item::Enum(i) if has_attr(&i.attrs, "protein") => proteins.push(item.clone()),
            Item::Struct(i) if has_attr(&i.attrs, "protein") => proteins.push(item.clone()),
            Item::Impl(i) if has_attr(&i.attrs, "handler") => {
                handler_impl = Some(i.clone());
                if let Type::Path(tp) = &*i.self_ty {
                    service_struct_name = tp.path.segments.last().unwrap().ident.to_string();
                }
            }
            _ => {}
        }
    }

    if handler_impl.is_none() { panic!("No #[handler] found in cell '{}'", cell_name); }

    // ... Extraction logic matching handler macro ...
    // Note: Re-implementing extraction here since we are in the same file/crate context
    let mut methods = Vec::new();
    for item in handler_impl.unwrap().items {
        if let syn::ImplItem::Fn(m) = item {
             let name = m.sig.ident.clone();
             let mut args = Vec::new();
             for arg in m.sig.inputs {
                if let FnArg::Typed(pat) = arg {
                    if let Pat::Ident(id) = *pat.pat {
                        // Normalize argument type for client call
                        let norm = normalize_ty(&pat.ty);
                        args.push((id.ident, norm));
                    }
                }
             }
             let ret = match m.sig.output {
                ReturnType::Default => syn::parse_quote! { () },
                ReturnType::Type(_, ty) => *ty,
             };
             let wire_ret = sanitize_return_type(&ret);
             methods.push((name, args, wire_ret));
        }
    }

    let protocol_name = format_ident!("{}Protocol", service_struct_name);
    let response_name = format_ident!("{}Response", service_struct_name);
    let client_struct = format_ident!("Client");
    let internal_mod_name = format_ident!("__{}_internal", module_name);

    let req_variants = methods.iter().map(|(n, args, _)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        let fields = args.iter().map(|(an, at)| quote! { #an: #at });
        quote! { #vname { #(#fields),* } }
    });

    let resp_variants = methods.iter().map(|(n, _, wret)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        quote! { #vname(#wret) }
    });

    let client_methods = methods.iter().map(|(n, args, wret)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        let args_sig = args.iter().map(|(an, at)| quote! { #an: #at });
        let args_struct = args.iter().map(|(an, _)| quote! { #an });
        quote! {
            pub async fn #n(&mut self, #(#args_sig),*) -> ::anyhow::Result<#wret> {
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

    let expanded = quote! {
        #[allow(non_snake_case, dead_code)]
        pub mod #internal_mod_name {
            use super::*;
            use cell_sdk::protein;
            use ::cell_sdk::serde::{Deserialize, Serialize};

            const _: &[u8] = include_bytes!(#dna_path_str);

            #(#proteins)*

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
                    Ok(Self { conn: ::cell_sdk::Synapse::grow(#cell_name).await? })
                }
                
                pub fn connection(&mut self) -> &mut ::cell_sdk::Synapse { &mut self.conn }
                #(#client_methods)*
            }
        }
        pub use #internal_mod_name::#client_struct as #module_name; 
    };

    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn protein(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let expanded = quote! {
        #[derive(
            ::cell_sdk::serde::Serialize,
            ::cell_sdk::serde::Deserialize,
            ::cell_sdk::rkyv::Archive,
            ::cell_sdk::rkyv::Serialize,
            ::cell_sdk::rkyv::Deserialize,
            Clone, Debug, PartialEq
        )]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(check_bytes)]
        #[archive(crate = "::cell_sdk::rkyv")]
        #input
    };
    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn service(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    TokenStream::from(quote! { #input })
}