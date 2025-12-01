// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

//! # Cell Macros
//!
//! This crate provides the procedural macros that power the Cell biological computing substrate.

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use serde::{Deserialize, Serialize};
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, DeriveInput, ItemImpl, LitStr, Token, Type};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CellGenome {
    name: String,
    fingerprint: u64,
    methods: Vec<MethodSchema>,
    types: Vec<TypeSchema>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct MethodSchema {
    name: String,
    inputs: Vec<(String, TypeRef)>,
    output: TypeRef,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TypeSchema {
    name: String,
    kind: TypeKind,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
enum TypeKind {
    Struct {
        fields: Vec<(String, TypeRef)>,
    },
    Enum {
        variants: Vec<(String, Vec<TypeRef>)>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
enum TypeRef {
    Named(String),
    Primitive(Primitive),
    Vec(Box<TypeRef>),
    Option(Box<TypeRef>),
    Result(Box<TypeRef>, Box<TypeRef>),
    Unit,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
enum Primitive {
    String,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Bool,
}

#[proc_macro_attribute]
pub fn protein(_attr: TokenStream, item: TokenStream) -> TokenStream {
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
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);

    let service_name = match &*input.self_ty {
        Type::Path(tp) => tp.path.segments.last().unwrap().ident.clone(),
        _ => panic!("Handler must implement a struct"),
    };

    struct MethodInfo {
        name: syn::Ident,
        args: Vec<(syn::Ident, Type)>,
        return_ty: Type,
    }

    let mut methods = Vec::new();
    for item in &input.items {
        if let syn::ImplItem::Fn(method) = item {
            let name = method.sig.ident.clone();

            let args: Vec<(syn::Ident, Type)> = method
                .sig
                .inputs
                .iter()
                .filter_map(|arg| {
                    if let syn::FnArg::Typed(pat) = arg {
                        if let syn::Pat::Ident(id) = &*pat.pat {
                            return Some((id.ident.clone(), *pat.ty.clone()));
                        }
                    }
                    None
                })
                .collect();

            let return_ty = match &method.sig.output {
                syn::ReturnType::Default => syn::parse_quote! { () },
                syn::ReturnType::Type(_, ty) => *ty.clone(),
            };

            methods.push(MethodInfo {
                name,
                args,
                return_ty,
            });
        }
    }

    let protocol_name = format_ident!("{}Protocol", service_name);
    let response_name = format_ident!("{}Response", service_name);

    let req_variants = methods.iter().map(|m| {
        let vname = format_ident!("{}", to_pascal_case(&m.name.to_string()));
        let fields = m.args.iter().map(|(n, t)| {
            let ty_owned = normalize_protocol_type(t);
            quote! { #n: #ty_owned }
        });
        quote! { #vname { #(#fields),* } }
    });

    let resp_variants = methods.iter().map(|m| {
        let vname = format_ident!("{}", to_pascal_case(&m.name.to_string()));
        let success_ty = extract_success_type(&m.return_ty);
        quote! { #vname(Result<#success_ty, String>) }
    });

    let dispatch_arms = methods.iter().map(|m| {
        let vname = format_ident!("{}", to_pascal_case(&m.name.to_string()));
        let fname = &m.name;
        let field_names: Vec<_> = m.args.iter().map(|(n, _)| n).collect();
        let success_ty = extract_success_type(&m.return_ty);

        let arg_handlers = m.args.iter().map(|(name, ty)| {
            if is_reference(ty) {
                quote! { let #name = #name; }
            } else {
                quote! {
                    let #name = {
                        let mut deser = ::cell_sdk::rkyv::de::deserializers::SharedDeserializeMap::new();
                        ::cell_sdk::rkyv::Deserialize::deserialize(#name, &mut deser)
                            .map_err(|e| ::anyhow::anyhow!("Deserialization failed: {:?}", e))?
                    };
                }
            }
        });

        quote! {
            ArchivedProtocol::#vname { #(#field_names),* } => {
                #(#arg_handlers)*
                let result = self.#fname(#(#field_names),*).await;
                let wire_result: Result<#success_ty, String> = result.map_err(|e| e.to_string());
                Ok(#response_name::#vname(wire_result))
            }
        }
    });

    let genome_json = "{}";

    let expanded = quote! {
        #input

        #[derive(
            ::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize,
            ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize
            // Removed explicit ::std::marker::Send as it's not a derive macro
        )]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(check_bytes)]
        #[archive(crate = "::cell_sdk::rkyv")]
        pub enum #protocol_name {
            #(#req_variants),*
        }

        #[derive(
            ::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize,
            ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize
        )]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(check_bytes)]
        #[archive(crate = "::cell_sdk::rkyv")]
        pub enum #response_name {
            #(#resp_variants),*
        }

        impl #service_name {
            pub const CELL_GENOME: &'static str = #genome_json;
            pub const SCHEMA_FINGERPRINT: u64 = 0x12345678;

            pub async fn serve(self, name: &str) -> ::anyhow::Result<()> {
                let service = std::sync::Arc::new(self);

                ::cell_sdk::Membrane::bind::<_, #protocol_name, #response_name>(
                    name,
                    move |archived_req| {
                        let svc = service.clone();
                        // Explicitly box the future to erase the return type and allow lifetime dependency
                        Box::pin(async move {
                            svc.dispatch(archived_req).await
                        })
                    },
                    Some(Self::CELL_GENOME.to_string())
                ).await
            }

            async fn dispatch(
                &self,
                req: &<#protocol_name as ::cell_sdk::rkyv::Archive>::Archived
            ) -> ::anyhow::Result<#response_name> {
                // Fix: Use type alias instead of 'use' for associated type
                type ArchivedProtocol = <#protocol_name as ::cell_sdk::rkyv::Archive>::Archived;
                match req {
                    #(#dispatch_arms),*
                }
            }
        }
    };

    TokenStream::from(expanded)
}

struct CellRemoteInput {
    name: syn::Ident,
    _eq: Token![=],
    address: LitStr,
}

impl Parse for CellRemoteInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(CellRemoteInput {
            name: input.parse()?,
            _eq: input.parse()?,
            address: input.parse()?,
        })
    }
}

#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    let CellRemoteInput { name, address, .. } = parse_macro_input!(input as CellRemoteInput);
    let address_str = address.value();

    let client_code = quote! {
        pub struct #name {
            conn: ::cell_sdk::Synapse,
        }

        impl #name {
            pub async fn connect() -> ::anyhow::Result<Self> {
                let conn = ::cell_sdk::Synapse::grow(#address_str).await?;
                Ok(Self { conn })
            }

            pub fn connection(&mut self) -> &mut ::cell_sdk::Synapse {
                &mut self.conn
            }
        }
    };

    TokenStream::from(client_code)
}

fn to_pascal_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn is_reference(ty: &Type) -> bool {
    matches!(ty, Type::Reference(_))
}

fn extract_success_type(ty: &Type) -> Type {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            if seg.ident == "Result" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return inner.clone();
                    }
                }
            }
        }
    }
    ty.clone()
}

fn normalize_protocol_type(ty: &Type) -> Type {
    match ty {
        Type::Reference(r) => {
            let inner = *r.elem.clone();
            if let Type::Path(tp) = &inner {
                if let Some(seg) = tp.path.segments.last() {
                    if seg.ident == "Archived" {
                        if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                            if let Some(syn::GenericArgument::Type(t)) = args.args.first() {
                                return t.clone();
                            }
                        }
                    }
                }
            }
            inner
        }
        _ => ty.clone(),
    }
}
