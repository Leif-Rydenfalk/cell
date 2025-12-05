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

// ============================================================================
//  1. CORE UTILITIES
// ============================================================================

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

fn is_zero_copy_ref(ty: &Type) -> bool {
    if let Type::Reference(type_ref) = ty {
        if let Type::Path(type_path) = &*type_ref.elem {
            if let Some(segment) = type_path.path.segments.last() {
                return segment.ident == "Archived";
            }
        }
    }
    false
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

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| {
        let p = a.path(); 
        if p.is_ident(name) { return true; }
        if p.segments.len() == 2 {
            let first = &p.segments[0].ident;
            let second = &p.segments[1].ident;
            return (first == "cell" || first == "cell_sdk") && second == name;
        }
        false
    })
}

struct ServiceMethod {
    name: syn::Ident,
    args: Vec<(syn::Ident, Type)>,      
    norm_args: Vec<(syn::Ident, Type)>, 
    ret: Type,                          
    wire_ret: Type,                     
}

fn extract_methods(items: &[syn::ImplItem]) -> Vec<ServiceMethod> {
    let mut methods = Vec::new();
    for item in items {
        if let syn::ImplItem::Fn(m) = item {
            if m.sig.asyncness.is_none() {
                panic!("Cell handler methods must be async: {}", m.sig.ident);
            }

            let name = m.sig.ident.clone();
            let mut args = Vec::new();
            let mut norm_args = Vec::new();

            for arg in &m.sig.inputs {
                if let FnArg::Typed(pat) = arg {
                    if let Pat::Ident(id) = &*pat.pat {
                        let original_ty = *pat.ty.clone();
                        let normalized_ty = normalize_ty(&original_ty);
                        
                        args.push((id.ident.clone(), original_ty));
                        norm_args.push((id.ident.clone(), normalized_ty));
                    }
                }
            }

            let ret = match &m.sig.output {
                ReturnType::Default => syn::parse_quote! { () },
                ReturnType::Type(_, ty) => *ty.clone(),
            };

            let wire_ret = sanitize_return_type(&ret);

            methods.push(ServiceMethod { name, args, norm_args, ret, wire_ret });
        }
    }
    methods
}

// ============================================================================
//  2. SERVER SIDE: #[handler]
// ============================================================================

#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    let self_ty = &input.self_ty;
    let service_name = match &**self_ty {
        Type::Path(tp) => tp.path.segments.last().unwrap().ident.clone(),
        _ => panic!("Handler must implement a struct"),
    };

    let methods = extract_methods(&input.items);
    let protocol_name = format_ident!("{}Protocol", service_name);
    let response_name = format_ident!("{}Response", service_name);

    let req_variants = methods.iter().map(|m| {
        let vname = format_ident!("{}", m.name.to_string().to_case(Case::Pascal));
        let fields = m.norm_args.iter().map(|(n, t)| quote! { #n: #t });
        quote! { #vname { #(#fields),* } }
    });

    let resp_variants = methods.iter().map(|m| {
        let vname = format_ident!("{}", m.name.to_string().to_case(Case::Pascal));
        let ret_ty = &m.wire_ret;
        quote! { #vname(#ret_ty) }
    });

    let dispatch_arms = methods.iter().map(|m| {
        let vname = format_ident!("{}", m.name.to_string().to_case(Case::Pascal));
        let fname = &m.name;
        let field_names: Vec<_> = m.args.iter().map(|(n, _)| n).collect();
        
        let arg_prep = m.args.iter().zip(m.norm_args.iter()).map(|((name, orig_ty), (_, _norm_ty))| {
            if is_zero_copy_ref(orig_ty) {
                quote! { let #name = #name; }
            } else {
                quote! {
                    let #name = {
                        let mut deser = ::cell_sdk::rkyv::de::deserializers::SharedDeserializeMap::new();
                        ::cell_sdk::rkyv::Deserialize::deserialize(#name, &mut deser)
                            .map_err(|e| ::anyhow::anyhow!("Deserialization failed for argument '{}': {:?}", stringify!(#name), e))?
                    };
                }
            }
        });

        let result_mapping = if m.wire_ret != m.ret {
            quote! {
                let wire_result = match result {
                    Ok(val) => Ok(val),
                    Err(e) => Err(format!("{:?}", e)),
                };
            }
        } else {
            quote! { let wire_result = result; }
        };

        quote! {
            ArchivedProtocol::#vname { #(#field_names),* } => {
                #(#arg_prep)*
                let result = self.#fname(#(#field_names),*).await;
                #result_mapping
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
            pub const SCHEMA_FINGERPRINT: u64 = 0xDEADBEEF;

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

// ============================================================================
//  3. CLIENT SIDE: cell_remote!
// ============================================================================

#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();
    let parts: Vec<&str> = input_str.split('=').collect();
    if parts.len() != 2 { panic!("Usage: cell_remote!(Module = \"cell_name\")"); }
    
    let module_name = format_ident!("{}", parts[0].trim());
    let cell_name = parts[1].trim().trim_matches(|c| c == '"' || c == ' ');

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
    
    let methods = extract_methods(&handler_impl.unwrap().items);
    let protocol_name = format_ident!("{}Protocol", service_struct_name);
    let response_name = format_ident!("{}Response", service_struct_name);
    let client_struct = format_ident!("Client");
    
    // FIX 1: Internal module name to prevent collision with struct alias
    let internal_mod_name = format_ident!("__{}_internal", module_name);

    let req_variants = methods.iter().map(|m| {
        let vname = format_ident!("{}", m.name.to_string().to_case(Case::Pascal));
        let fields = m.norm_args.iter().map(|(n, t)| quote! { #n: #t });
        quote! { #vname { #(#fields),* } }
    });

    let resp_variants = methods.iter().map(|m| {
        let vname = format_ident!("{}", m.name.to_string().to_case(Case::Pascal));
        let ret_ty = &m.wire_ret;
        quote! { #vname(#ret_ty) }
    });

    let client_methods = methods.iter().map(|m| {
        let fname = &m.name;
        let vname = format_ident!("{}", m.name.to_string().to_case(Case::Pascal));
        let args_sig = m.norm_args.iter().map(|(n, t)| quote! { #n: #t });
        let args_struct = m.norm_args.iter().map(|(n, _)| quote! { #n });
        let ret_ty = &m.wire_ret;

        quote! {
            pub async fn #fname(&mut self, #(#args_sig),*) -> ::anyhow::Result<#ret_ty> {
                let req = #protocol_name::#vname { #(#args_struct),* };
                let resp = self.conn.fire::<#protocol_name, #response_name>(&req).await?;
                let val = resp.deserialize()?;
                match val {
                    #response_name::#vname(res) => Ok(res),
                    _ => Err(::anyhow::anyhow!("Protocol Mismatch: Expected variant {}", stringify!(#vname))),
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
            
            // FIX 2: Connect is now part of the Struct impl
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