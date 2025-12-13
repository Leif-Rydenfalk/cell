// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::{Parse, ParseStream}, parse_macro_input, Item, ItemImpl, Type, FnArg, Pat, 
    ReturnType, Token, Ident, LitStr, GenericArgument, PathArguments, Attribute
};
use convert_case::{Case, Casing};
use std::path::PathBuf;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

mod test;
mod coordination;

// --- HELPERS ---

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
                    if let Some(GenericArgument::Type(inner)) = args.args.first() {
                        return inner.clone();
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
                if segment.ident == "Archived" {
                    return true;
                }
            }
        }
    }
    false
}

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path().is_ident(name) || (a.path().segments.len() == 2 && a.path().segments[1].ident == name))
}

fn locate_dna(cell_name: &str) -> PathBuf {
    if let Ok(p) = std::env::var("CELL_SCHEMA_PATH") {
        let path = PathBuf::from(p).join(format!("{}.rs", cell_name));
        if path.exists() { return path; }
    }
    
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("No MANIFEST_DIR");
    let current_dir = std::path::Path::new(&manifest);
    
    // Check local source if self-referencing
    let local_src = current_dir.join("src/main.rs");
    if local_src.exists() && current_dir.ends_with(cell_name) {
        return local_src;
    }

    let potential_roots = [
        current_dir.to_path_buf(),
        current_dir.parent().unwrap_or(current_dir).to_path_buf(),
        current_dir.join("../"), 
        current_dir.join("../../"), 
        current_dir.join("../../../"), 
    ];

    for root in potential_roots {
        let check_paths = [
            root.join("cells").join(cell_name).join("src/main.rs"),
            root.join("examples").join("cell-market").join(cell_name).join("src/main.rs"),
            root.join("examples").join("cell-market-bench").join("cells").join(cell_name).join("src/main.rs"),
            root.join("examples").join("cell-tissue").join(cell_name).join("src/main.rs"),
            root.join("examples").join("cell-schema-sync").join(cell_name).join("src/main.rs"),
            root.join("cells").join(cell_name.replace("-raft", "")).join("src/main.rs"),
        ];
        for p in check_paths {
            if p.exists() { return p; }
        }
    }
    PathBuf::from("DNA_NOT_FOUND") 
}

// --- CELL_REMOTE MACRO ---

struct CellRemoteArgs {
    module_name: Ident,
    cell_name: String,
}
impl Parse for CellRemoteArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
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
    let cell_name = args.cell_name;

    // 1. Resolve socket path at compile time
    let socket_path = match cell_build::resolve(&cell_name) {
        Ok(path) => path,
        Err(e) => panic!("Failed to resolve dependency '{}': {}", cell_name, e),
    };

    // 2. Load DNA
    let dna_path = locate_dna(&cell_name);
    if dna_path.to_string_lossy() == "DNA_NOT_FOUND" {
        panic!("Could not locate DNA for '{}'.", cell_name);
    }

    let file = match cell_build::load_and_flatten_source(&dna_path) {
        Ok(f) => f,
        Err(e) => panic!("Failed to load DNA: {}", e),
    };

    // 3. Reflect Structure
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
                Item::Mod(m) => { if let Some((_, items)) = &m.content { find_items(items, proteins, handler, service_name); } }
                _ => {}
            }
        }
    }
    find_items(&file.items, &mut proteins, &mut handler_impl, &mut service_struct_name);
    if handler_impl.is_none() { panic!("No #[handler] found in {}", cell_name); }

    let mut methods = Vec::new();
    for item in handler_impl.unwrap().items.iter() {
        if let syn::ImplItem::Fn(m) = item {
             let name = m.sig.ident.clone();
             let mut args = Vec::new();
             for arg in &m.sig.inputs {
                if let FnArg::Typed(pat) = arg {
                    if let Pat::Ident(id) = &*pat.pat {
                        args.push((id.ident.clone(), normalize_ty(&pat.ty)));
                    }
                }
             }
             let ret = match &m.sig.output { ReturnType::Default => syn::parse_quote! { () }, ReturnType::Type(_, ty) => *ty.clone() };
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
        quote! {
            pub async fn #n(&mut self, #(#args_sig),*) -> ::std::result::Result<#wret, ::cell_sdk::CellError> {
                let req = #protocol_name::#vname { #(#args_struct),* };
                let resp = self.conn.fire::<#protocol_name, #response_name>(&req).await?;
                let val = resp.deserialize().map_err(|_| ::cell_sdk::CellError::SerializationFailure)?;
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

            // Baked-in Socket Path from Resolution
            pub const SOCKET_PATH: &'static str = #socket_path;
            
            // Dependency declaration
            pub const DEPENDENCIES: &[&str] = &[#cell_name];

            #(#proteins)*

            #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
            #[serde(crate = "::cell_sdk::serde")]
            #[archive(crate = "::cell_sdk::rkyv")]
            pub enum #protocol_name { #(#req_variants),* }

            #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
            #[serde(crate = "::cell_sdk::serde")]
            #[archive(check_bytes)]
            #[archive(crate = "::cell_sdk::rkyv")]
            pub enum #response_name { #(#resp_variants),* }

            pub struct #client_struct { conn: ::cell_sdk::Synapse }
            impl #client_struct {
                pub fn new(conn: ::cell_sdk::Synapse) -> Self { Self { conn } }
                
                // Enhanced connect: waits for dependencies
                pub async fn connect() -> ::anyhow::Result<Self> { 
                    // Runtime wait for dependency readiness
                    ::cell_sdk::mesh::MeshBuilder::wait_for_dependencies(DEPENDENCIES).await?;
                    
                    // Connect directly to the baked-in socket path
                    let conn = ::cell_sdk::Synapse::connect_direct(SOCKET_PATH).await?;
                    Ok(Self { conn })
                }
                
                pub fn connection(&mut self) -> &mut ::cell_sdk::Synapse { &mut self.conn }
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
        let fields = args.iter().map(|(an, at)| { let norm = normalize_ty(at); quote! { #an: #norm } });
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
            if is_zero_copy_ref(at) { quote! { let #an = #an; } } 
            else { quote! { let #an = ::cell_sdk::rkyv::Deserialize::deserialize(#an, &mut ::cell_sdk::rkyv::de::deserializers::SharedDeserializeMap::new()).map_err(|e| ::anyhow::anyhow!("Deserialization failed for {}: {:?}", stringify!(#an), e))?; } }
        });
        
        quote! {
            ArchivedProtocol::#vname { #(#field_names),* } => {
                #(#arg_preps)*
                let result = self.#n(#(#field_names),*).await?;
                Ok(#response_name::#vname(result))
            }
        }
    });

    let mut hasher = DefaultHasher::new();
    service_name.to_string().hash(&mut hasher);
    for (n, _, _, _) in &methods { n.to_string().hash(&mut hasher); }
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
                        Box::pin(async move { svc.dispatch(archived_req).await })
                    },
                    name,
                ).await
            }
            async fn dispatch(&self, req: &<#protocol_name as ::cell_sdk::rkyv::Archive>::Archived) -> ::anyhow::Result<#response_name> {
                type ArchivedProtocol = <#protocol_name as ::cell_sdk::rkyv::Archive>::Archived;
                match req { #(#dispatch_arms),* }
            }
        }
    };
    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn cell_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    test::cell_test_impl(item)
}

#[proc_macro_attribute]
pub fn cell_macro(_attr: TokenStream, item: TokenStream) -> TokenStream { item }

// --- THE MYCELIAL SCHEMA SYNC MACRO ---

struct ExpandArgs {
    cell_name: String,
    macro_name: String,
}

impl Parse for ExpandArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let cell_name_lit: LitStr = input.parse()?;
        input.parse::<Token![,]>()?;
        let macro_name_lit: LitStr = input.parse()?;
        Ok(ExpandArgs {
            cell_name: cell_name_lit.value(),
            macro_name: macro_name_lit.value(),
        })
    }
}

#[proc_macro_attribute]
pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ExpandArgs);
    let item_struct = parse_macro_input!(item as syn::ItemStruct);

    // 1. Resolve connection to target cell at compile time
    // This requires the cell to be resolvable/running.
    let _socket_path = match cell_build::resolve(&args.cell_name) {
        Ok(p) => p,
        Err(e) => panic!("Failed to resolve macro provider '{}': {}", args.cell_name, e),
    };

    // 2. Extract Context
    let struct_name = item_struct.ident.to_string();
    let mut fields = Vec::new();
    
    // If it's a unit struct (stub), fields will be empty
    if let syn::Fields::Named(named) = &item_struct.fields {
        for field in &named.named {
            let name = field.ident.as_ref().unwrap().to_string();
            let ty = quote! { #field.ty }.to_string();
            fields.push((name, ty));
        }
    }

    let context = cell_model::macro_coordination::ExpansionContext {
        struct_name,
        fields,
        attributes: vec![],
        other_cells: vec![],
    };

    // 3. Coordinate with remote cell
    let coordinator = coordination::MacroCoordinator::new(&args.cell_name);
    
    let generated_code = match coordinator.coordinate_expansion(&args.macro_name, context) {
        Ok(code) => code,
        Err(e) => panic!("Macro expansion failed from '{}': {}", args.cell_name, e),
    };

    // 4. Parse the returned code into a TokenStream
    match syn::parse_str::<proc_macro2::TokenStream>(&generated_code) {
        Ok(stream) => TokenStream::from(stream),
        Err(e) => panic!("Invalid code generated by '{}': {}", args.cell_name, e),
    }
}