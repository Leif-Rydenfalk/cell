// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::{
    parse::{Parse, ParseStream}, parse_macro_input, DeriveInput, Item, ItemImpl, Type, FnArg, Pat, 
    ReturnType, spanned::Spanned, Attribute, GenericArgument, PathArguments, Token, Ident, LitStr, LitBool
};
use convert_case::{Case, Casing};
use std::path::PathBuf;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use cell_transport::coordination::MacroCoordinator;
use cell_model::macro_coordination::ExpansionContext;

// Helpers
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
    // We preserve Result<T, E> to support user-defined errors on the wire.
    ty.clone()
}

fn is_zero_copy_ref(ty: &Type) -> bool {
    if let Type::Reference(type_ref) = ty {
        if let Type::Path(type_path) = &*type_ref.elem {
            if let Some(segment) = type_path.path.segments.last() {
                let s = segment.ident.to_string();
                if s == "Archived" {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(GenericArgument::Type(inner)) = args.args.first() {
                            if let Type::Path(inner_path) = inner {
                                if let Some(inner_seg) = inner_path.path.segments.last() {
                                    if inner_seg.ident == "Vec" || inner_seg.ident == "String" {
                                        return false;
                                    }
                                }
                            }
                        }
                    }
                    return true;
                }
            }
        }
    }
    false
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

// --- IPC Expansion Macro ---

struct ExpandArgs {
    cell_name: String,
    macro_name: String,
}

impl Parse for ExpandArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let cell_name = if input.peek(LitStr) {
            input.parse::<LitStr>()?.value()
        } else {
            input.parse::<Ident>()?.to_string()
        };
        
        input.parse::<Token![,]>()?;
        
        let macro_name = if input.peek(LitStr) {
            input.parse::<LitStr>()?.value()
        } else {
            input.parse::<Ident>()?.to_string()
        };
        
        Ok(ExpandArgs { cell_name, macro_name })
    }
}

#[proc_macro_attribute]
pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ExpandArgs);
    let input_item = parse_macro_input!(item as Item);
    
    let context = match extract_context(&input_item) {
        Ok(ctx) => ctx,
        Err(e) => return e.to_compile_error().into(),
    };
    
    // Connect to Cell
    let coordinator = MacroCoordinator::new(&args.cell_name);
    
    let result = coordinator.coordinate_expansion(&args.macro_name, context);
    
    match result {
        Ok(code) => {
            use std::str::FromStr;
            match proc_macro2::TokenStream::from_str(&code) {
                Ok(ts) => TokenStream::from(ts),
                Err(e) => {
                    let msg = format!("Failed to parse expanded code from cell '{}': {}", args.cell_name, e);
                    syn::Error::new(input_item.span(), msg).to_compile_error().into()
                }
            }
        }
        Err(e) => {
             let msg = format!(
                 "Cell Expansion Error: Could not talk to cell '{}'. Ensure it is running!\nDetails: {}", 
                 args.cell_name, e
             );
             syn::Error::new(input_item.span(), msg).to_compile_error().into()
        }
    }
}

fn extract_context(item: &Item) -> syn::Result<ExpansionContext> {
    match item {
        Item::Struct(s) => {
            let struct_name = s.ident.to_string();
            let mut fields = Vec::new();
            
            for field in &s.fields {
                let fname = field.ident.as_ref()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "unnamed".to_string());
                let ftype = field.ty.to_token_stream().to_string();
                fields.push((fname, ftype));
            }
            
            let attributes = s.attrs.iter().map(|a| a.to_token_stream().to_string()).collect();
            
            Ok(ExpansionContext {
                struct_name,
                fields,
                attributes,
                other_cells: vec![],
            })
        }
        _ => Err(syn::Error::new(item.span(), "Only structs are supported for cell expansion currently")),
    }
}

// --- Cell Remote ---

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
    
    let dispatch_arms = methods.iter().map(|(n, args, _, _)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        let field_names: Vec<_> = args.iter().map(|(an, _)| an).collect();
        
        let arg_preps = args.iter().map(|(an, at)| {
            if is_zero_copy_ref(at) {
                quote! { let #an = #an; }
            } else {
                quote! {
                    let #an = ::cell_sdk::rkyv::Deserialize::deserialize(
                        #an, 
                        &mut ::cell_sdk::rkyv::de::deserializers::SharedDeserializeMap::new()
                    ).map_err(|e| ::anyhow::anyhow!("Deserialization failed for argument '{}': {:?}", stringify!(#an), e))?;
                }
            }
        });

        quote! {
            ArchivedProtocol::#vname { #(#field_names),* } => {
                #(#arg_preps)*
                let result = self.#n(#(#field_names),*).await;
                // Directly wrap the result (including App Errors) into the response enum
                Ok(#response_name::#vname(result))
            }
        }
    });

    let mut hasher = DefaultHasher::new();
    service_name.to_string().hash(&mut hasher);
    for (n, _, _, _) in &methods {
        n.to_string().hash(&mut hasher);
    }
    let fingerprint = hasher.finish();

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
            pub const SCHEMA_FINGERPRINT: u64 = #fingerprint;

            pub async fn serve(self, name: &str) -> ::anyhow::Result<()> {
                let service = std::sync::Arc::new(self);
                ::cell_sdk::Runtime::ignite::<_, #protocol_name, #response_name>(
                    move |archived_req| {
                        let svc = service.clone();
                        Box::pin(async move {
                            svc.dispatch(archived_req).await
                        })
                    },
                    name,
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

struct CellRemoteArgs {
    module_name: Ident,
    cell_name: String,
    import_macros: bool,
}

impl Parse for CellRemoteArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let module_name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let cell_name_lit: LitStr = input.parse()?;
        let cell_name = cell_name_lit.value();
        
        let mut import_macros = false;
        
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if !input.is_empty() {
                let key: Ident = input.parse()?;
                if key == "import_macros" {
                    input.parse::<Token![=]>()?;
                    let val: LitBool = input.parse()?;
                    import_macros = val.value;
                }
            }
        }
        
        Ok(CellRemoteArgs { module_name, cell_name, import_macros })
    }
}

#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as CellRemoteArgs);
    let module_name = args.module_name;
    let cell_name = args.cell_name;

    let out_dir = std::env::var("OUT_DIR").ok();
    if let Some(ref dir) = out_dir {
        let path = PathBuf::from(dir).join(format!("{}_client.rs", cell_name));
        if path.exists() {
            let path_str = path.to_str().unwrap();
            return TokenStream::from(quote! {
                include!(#path_str);
                pub use #cell_name::Client as #module_name;
            });
        }
    }

    let dna_path = locate_dna(&cell_name);
    let dna_path_str = dna_path.to_str().expect("Invalid path");
    
    let file = match cell_build::load_and_flatten_source(&dna_path) {
        Ok(f) => f,
        Err(e) => panic!("Failed to flatten DNA source: {}", e),
    };

    let mut proteins = Vec::new();
    let mut handler_impl = None;
    let mut service_struct_name = String::new();

    fn find_items<'a>(items: &'a [Item], proteins: &mut Vec<&'a Item>, handler: &mut Option<&'a syn::ItemImpl>, service_name: &mut String) {
        for item in items {
            match item {
                Item::Enum(i) if has_attr(&i.attrs, "protein") => proteins.push(item),
                Item::Struct(i) if has_attr(&i.attrs, "protein") => proteins.push(item),
                Item::Impl(i) if has_attr(&i.attrs, "handler") => {
                    *handler = Some(i);
                    if let Type::Path(tp) = &*i.self_ty {
                        *service_name = tp.path.segments.last().unwrap().ident.to_string();
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, items)) = &m.content {
                        find_items(items, proteins, handler, service_name);
                    }
                }
                _ => {}
            }
        }
    }

    find_items(&file.items, &mut proteins, &mut handler_impl, &mut service_struct_name);

    if handler_impl.is_none() { panic!("No #[handler] found in cell '{}'", cell_name); }

    let mut methods = Vec::new();
    for item in handler_impl.unwrap().items.iter() {
        if let syn::ImplItem::Fn(m) = item {
             let name = m.sig.ident.clone();
             let mut args = Vec::new();
             for arg in &m.sig.inputs {
                if let FnArg::Typed(pat) = arg {
                    if let Pat::Ident(id) = &*pat.pat {
                        let norm = normalize_ty(&pat.ty);
                        args.push((id.ident.clone(), norm));
                    }
                }
             }
             let ret = match &m.sig.output {
                ReturnType::Default => syn::parse_quote! { () },
                ReturnType::Type(_, ty) => *ty.clone(),
             };
             let wire_ret = sanitize_return_type(&ret);
             methods.push((name, args, wire_ret));
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

    let resp_variants = methods.iter().map(|(n, _, wret)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        quote! { #vname(#wret) }
    });

    let client_methods = methods.iter().map(|(n, args, wret)| {
        let vname = format_ident!("{}", n.to_string().to_case(Case::Pascal));
        let args_sig = args.iter().map(|(an, at)| quote! { #an: #at });
        let args_struct = args.iter().map(|(an, _)| quote! { #an });
        
        // This generates:
        // Result<Result<u64, LedgerError>, CellError>
        quote! {
            pub async fn #n(&mut self, #(#args_sig),*) -> ::std::result::Result<#wret, ::cell_sdk::CellError> {
                let req = #protocol_name::#vname { #(#args_struct),* };
                
                // 1. System Error Check (Transport/Substrate failure)
                let resp = self.conn.fire::<#protocol_name, #response_name>(&req).await?;
                
                // 2. Serialization Check
                let val = resp.deserialize().map_err(|_| ::cell_sdk::CellError::SerializationFailure)?;
                
                // 3. Protocol Check
                match val {
                    #response_name::#vname(res) => Ok(res),
                    _ => Err(::cell_sdk::CellError::VersionMismatch),
                }
            }
        }
    });

    let expanded = quote! {
        #[allow(non_snake_case, dead_code)]
        pub mod #module_name {
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

            pub async fn connect() -> ::anyhow::Result<#client_struct> {
                #client_struct::connect().await
            }
        }
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

#[proc_macro_attribute]
pub fn cell_macro(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Just pass through - the extraction happens in cell-build
    item
}