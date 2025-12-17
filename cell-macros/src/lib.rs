// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse::Parse, parse_macro_input, ItemImpl, Type, FnArg, Pat, ReturnType, Token, Ident, LitStr, GenericArgument, PathArguments, Item};
use convert_case::{Case, Casing};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

mod expand;
mod test;

// === CELL_REMOTE ===
struct CellRemoteArgs {
    module_name: Ident,
    cell_name: String,
}

impl Parse for CellRemoteArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let module_name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let cell_name_lit: LitStr = input.parse()?;
        Ok(CellRemoteArgs { module_name, cell_name: cell_name_lit.value() })
    }
}

#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as CellRemoteArgs);
    let module_name = args.module_name;
    let cell_name = &args.cell_name;

    // 1. Fetch Source (Filesystem Only - No RPC)
    let source_code = fetch_remote_source_or_fallback(cell_name);
    
    // 2. Extract Methods
    let methods = extract_handler_methods(&source_code);
    if methods.is_empty() {
        return syn::Error::new(
            module_name.span(), 
            format!("No #[handler] methods found for '{}'. Ensure source is accessible in Workspace or Registry.", cell_name)
        ).to_compile_error().into();
    }

    // 3. Extract Proteins
    let proteins = extract_proteins(&source_code);

    let protocol_name = format_ident!("{}Protocol", cell_name.to_case(Case::Pascal));
    let response_name = format_ident!("{}Response", cell_name.to_case(Case::Pascal));

    let req_variants: Vec<_> = methods.iter().map(|(name, args, _)| {
        let variant_name = format_ident!("{}", name.to_string().to_case(Case::Pascal));
        let fields: Vec<_> = args.iter().map(|(arg_name, arg_type)| quote! { #arg_name: #arg_type }).collect();
        quote! { #variant_name { #(#fields),* } }
    }).collect();

    let resp_variants: Vec<_> = methods.iter().map(|(name, _, ret_type)| {
        let variant_name = format_ident!("{}", name.to_string().to_case(Case::Pascal));
        quote! { #variant_name(#ret_type) }
    }).collect();

    let client_methods: Vec<_> = methods.iter().map(|(name, args, ret_type)| {
        let variant_name = format_ident!("{}", name.to_string().to_case(Case::Pascal));
        let arg_sigs: Vec<_> = args.iter().map(|(arg_name, arg_type)| quote! { #arg_name: #arg_type }).collect();
        let arg_names: Vec<_> = args.iter().map(|(arg_name, _)| arg_name).collect();

        // CHANGED: &mut self -> &self
        quote! {
            pub async fn #name(&self, #(#arg_sigs),*) -> ::anyhow::Result<#ret_type> {
                let req = #protocol_name::#variant_name { #(#arg_names),* };
                
                let resp_wrapper = self.conn.fire(&req).await
                    .map_err(|e| ::anyhow::anyhow!("RPC Error: {}", e))?;
                
                let resp_bytes = resp_wrapper.into_owned();
                
                if resp_bytes.is_empty() {
                     return Err(::anyhow::anyhow!("Empty response from cell"));
                }

                let archived = ::cell_sdk::rkyv::check_archived_root::<#response_name>(&resp_bytes)
                    .map_err(|e| ::anyhow::anyhow!("Validation Error: {}", e))?;
                
                let resp: #response_name = ::cell_sdk::rkyv::Deserialize::deserialize(
                    archived, 
                    &mut ::cell_sdk::rkyv::de::deserializers::SharedDeserializeMap::new()
                ).map_err(|e| ::anyhow::anyhow!("Deserialization Error: {}", e))?;
                    
                match resp {
                    #response_name::#variant_name(result) => Ok(result),
                    _ => Err(::anyhow::anyhow!("Protocol Mismatch")),
                }
            }
        }
    }).collect();

    let expanded = quote! {
        #[allow(non_snake_case)]
        pub mod #module_name {
            use ::cell_sdk::*;

            #(#proteins)*

            #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize)]
            #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
            #[archive(check_bytes)]
            #[serde(crate = "::cell_sdk::serde")]
            #[archive(crate = "::cell_sdk::rkyv")]
            pub enum #protocol_name { #(#req_variants),* }

            #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize)]
            #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
            #[archive(check_bytes)]
            #[serde(crate = "::cell_sdk::serde")]
            #[archive(crate = "::cell_sdk::rkyv")]
            pub enum #response_name { #(#resp_variants),* }

            pub struct Client { conn: ::cell_sdk::Synapse }

            impl Client {
                pub async fn connect() -> ::anyhow::Result<Self> {
                    let conn = ::cell_sdk::Synapse::grow(#cell_name).await?;
                    Ok(Self { conn })
                }
                pub fn new(conn: ::cell_sdk::Synapse) -> Self {
                    Self { conn }
                }
                #(#client_methods)*
            }
        }
    };
    TokenStream::from(expanded)
}

fn fetch_remote_source_or_fallback(cell_name: &str) -> String {
    // 1. Check Workspace (Monorepo)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let current = std::path::Path::new(&manifest_dir);
        let mut candidate_root = current;
        // Walk up a few levels to find the root
        for _ in 0..4 {
             let candidates = [
                candidate_root.join(cell_name).join("src/main.rs"),
                candidate_root.join("cells").join(cell_name).join("src/main.rs"),
                candidate_root.join("examples").join("cell-market-bench").join("cells").join(cell_name).join("src/main.rs"),
                candidate_root.join("examples").join("cell-market").join("cells").join(cell_name).join("src/main.rs"),
            ];
            
            for c in &candidates {
                if c.exists() {
                    return std::fs::read_to_string(c).unwrap_or_default();
                }
            }
            if let Some(p) = candidate_root.parent() {
                candidate_root = p;
            } else {
                break;
            }
        }
    }

    // 2. Check Registry
    let home = dirs::home_dir().expect("No HOME");
    let registry = home.join(".cell/registry").join(cell_name);
    let c = registry.join("src/main.rs");
    if c.exists() { return std::fs::read_to_string(c).unwrap_or_default(); }

    panic!("Could not find source for cell '{}'. It must be in the workspace or ~/.cell/registry", cell_name);
}

fn extract_proteins(src: &str) -> Vec<proc_macro2::TokenStream> {
    let syntax = syn::parse_file(src).unwrap_or_else(|_| syn::File { items: vec![], shebang: None, attrs: vec![] });
    let mut proteins = Vec::new();
    for item in syntax.items {
        match item {
            Item::Struct(mut s) => {
                if s.attrs.iter().any(|a| a.path().is_ident("protein")) {
                    s.attrs.retain(|a| !a.path().is_ident("protein"));
                    let tokens = quote! {
                        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize)]
                        #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
                        #[archive(check_bytes)]
                        #[serde(crate = "::cell_sdk::serde")]
                        #[archive(crate = "::cell_sdk::rkyv")]
                        #[derive(Clone, Debug, PartialEq)]
                        #s
                    };
                    proteins.push(tokens);
                }
            }
            Item::Enum(mut e) => {
                if e.attrs.iter().any(|a| a.path().is_ident("protein")) {
                    e.attrs.retain(|a| !a.path().is_ident("protein"));
                    let tokens = quote! {
                        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize)]
                        #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
                        #[archive(check_bytes)]
                        #[serde(crate = "::cell_sdk::serde")]
                        #[archive(crate = "::cell_sdk::rkyv")]
                        #[derive(Clone, Debug, PartialEq)]
                        #e
                    };
                    proteins.push(tokens);
                }
            }
            _ => {}
        }
    }
    proteins
}

fn extract_handler_methods(src: &str) -> Vec<(Ident, Vec<(Ident, Type)>, Type)> {
    let syntax = syn::parse_file(src).unwrap_or_else(|_| syn::File { items: vec![], shebang: None, attrs: vec![] });
    let mut methods = Vec::new();
    for item in syntax.items {
        if let syn::Item::Impl(i) = item {
            if i.attrs.iter().any(|a| a.path().is_ident("handler")) {
                for impl_item in i.items {
                    if let syn::ImplItem::Fn(m) = impl_item {
                        let name = m.sig.ident;
                        let args: Vec<_> = m.sig.inputs.iter().filter_map(|arg| {
                            if let FnArg::Typed(pt) = arg {
                                if let Pat::Ident(pi) = &*pt.pat {
                                    return Some((pi.ident.clone(), *pt.ty.clone()));
                                }
                            }
                            None
                        }).collect();
                        let ret = extract_ok_type(&m.sig.output);
                        methods.push((name, args, ret));
                    }
                }
            }
        }
    }
    methods
}

fn extract_ok_type(ret: &ReturnType) -> Type {
    match ret {
        ReturnType::Default => syn::parse_quote! { () },
        ReturnType::Type(_, ty) => {
            if let Type::Path(tp) = &**ty {
                if let Some(seg) = tp.path.segments.last() {
                    if seg.ident == "Result" {
                        if let PathArguments::AngleBracketed(args) = &seg.arguments {
                            if let Some(GenericArgument::Type(inner)) = args.args.first() {
                                return inner.clone();
                            }
                        }
                    }
                }
            }
            *ty.clone()
        }
    }
}

// === ATTRIBUTE MACROS ===

#[proc_macro_attribute]
pub fn service(_: TokenStream, item: TokenStream) -> TokenStream { item }

#[proc_macro_attribute]
pub fn protein(_: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::DeriveInput);
    let expanded = quote! {
        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize)]
        #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
        #[archive(check_bytes)]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(crate = "::cell_sdk::rkyv")]
        #[derive(Clone, Debug, PartialEq)]
        #input
    };
    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand::expand_impl(attr, item)
}

#[proc_macro_attribute]
pub fn handler(_: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    let self_ty = &input.self_ty;
    
    let service_name = match &**self_ty {
        Type::Path(p) => p.path.segments.last().unwrap().ident.clone(),
        _ => panic!("Handler must implement struct"),
    };

    let protocol_name = format_ident!("{}Protocol", service_name);
    let response_name = format_ident!("{}Response", service_name);
    let archived_protocol_name = format_ident!("Archived{}Protocol", service_name);

    let mut methods = Vec::new();
    for impl_item in &input.items {
        if let syn::ImplItem::Fn(m) = impl_item {
            let name = m.sig.ident.clone();
            let args: Vec<_> = m.sig.inputs.iter().filter_map(|arg| {
                if let FnArg::Typed(pt) = arg {
                    if let Pat::Ident(pi) = &*pt.pat {
                        return Some((pi.ident.clone(), *pt.ty.clone()));
                    }
                }
                None
            }).collect();
            let ret = extract_ok_type(&m.sig.output);
            methods.push((name, args, ret));
        }
    }

    let req_variants: Vec<_> = methods.iter().map(|(name, args, _)| {
        let variant = format_ident!("{}", name.to_string().to_case(Case::Pascal));
        let fields = args.iter().map(|(n, t)| quote! { #n: #t });
        quote! { #variant { #(#fields),* } }
    }).collect();

    let resp_variants: Vec<_> = methods.iter().map(|(name, _, ret)| {
        let variant = format_ident!("{}", name.to_string().to_case(Case::Pascal));
        quote! { #variant(#ret) }
    }).collect();

    let dispatch_arms: Vec<_> = methods.iter().map(|(name, args, _)| {
        let variant = format_ident!("{}", name.to_string().to_case(Case::Pascal));
        
        let field_names: Vec<_> = args.iter().map(|(n, _)| n).collect();
        let field_bindings: Vec<_> = field_names.iter().map(|n| quote!{ #n }).collect();
        
        let deserializers: Vec<_> = args.iter().map(|(n, _)| {
            quote! {
                let #n = ::cell_sdk::rkyv::Deserialize::deserialize(
                    #n, 
                    &mut ::cell_sdk::rkyv::de::deserializers::SharedDeserializeMap::new()
                ).map_err(|_| ::cell_sdk::CellError::SerializationFailure)?;
            }
        }).collect();
        
        let call_args = field_names;

        quote! {
            #archived_protocol_name::#variant { #(#field_bindings),* } => {
                #(#deserializers)*
                let result = self.#name(#(#call_args),*).await?;
                Ok(#response_name::#variant(result))
            }
        }
    }).collect();

    let mut hasher = DefaultHasher::new();
    service_name.to_string().hash(&mut hasher);
    let fingerprint = hasher.finish();

    let expanded = quote! {
        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize)]
        #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
        #[archive(check_bytes)]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(crate = "::cell_sdk::rkyv")]
        pub enum #protocol_name { #(#req_variants),* }

        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize)]
        #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
        #[archive(check_bytes)]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(crate = "::cell_sdk::rkyv")]
        pub enum #response_name { #(#resp_variants),* }

        #input

        impl #service_name {
            pub const SCHEMA_FINGERPRINT: u64 = #fingerprint;

            pub async fn serve(self, name: &str) -> ::anyhow::Result<()> {
                let service = std::sync::Arc::new(self);
                ::cell_sdk::Membrane::bind::<_, #protocol_name, #response_name>(
                    name,
                    move |archived_req| {
                        let svc = service.clone();
                        Box::pin(async move { svc.dispatch(archived_req).await })
                    },
                    None, None, None 
                ).await
            }

            async fn dispatch(&self, req: &#archived_protocol_name) -> ::anyhow::Result<#response_name> {
                match req {
                    #(#dispatch_arms),*
                }
            }
        }
    };
    TokenStream::from(expanded)
}