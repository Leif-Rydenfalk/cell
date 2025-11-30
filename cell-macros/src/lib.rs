//! # Cell Macros
//! 
//! This crate provides the procedural macros that power the Cell biological computing substrate.
//! It handles the "Nuclear Option" of compile-time reflection, schema generation, and 
//! zero-copy serialization implementations.

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, DeriveInput, ItemImpl, LitStr, Token, Type};

// =========================================================================
//  Internal Schema Representation
//  These structs represent the "Genome" of a cell. They are serialized to JSON
//  and embedded in the binary or sent over the wire for introspection.
// =========================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CellGenome {
    /// Name of the service (e.g., "RendererService")
    name: String,
    /// Cryptographic hash of the method signatures and types
    fingerprint: u64,
    /// List of RPC methods available
    methods: Vec<MethodSchema>,
    /// Definitions of complex types used in methods
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
    Struct { fields: Vec<(String, TypeRef)> },
    Enum { variants: Vec<(String, Vec<TypeRef>)> },
}

/// Recursive type definition for schema reflection
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
    String, U8, U16, U32, U64, I8, I16, I32, I64, F32, F64, Bool,
}

// Protocol used during Mitosis (Spawning) compile-time connection
#[derive(::rkyv::Archive, ::rkyv::Serialize, ::rkyv::Deserialize)]
#[archive(check_bytes)]
enum MitosisRequest {
    Spawn { cell_name: String },
}

// =========================================================================
//  MACRO: #[protein]
//  Decorates structs/enums to make them safe for zero-copy transport.
// =========================================================================
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
        // Use the crate-level re-exports to ensure paths resolve correctly
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(check_bytes)]
        #[archive(crate = "::cell_sdk::rkyv")]
        #input
    };

    TokenStream::from(expanded)
}

// =========================================================================
//  MACRO: #[service]
//  Marker for the main struct of a cell. Future-proofs for state management.
// =========================================================================
#[proc_macro_attribute]
pub fn service(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    TokenStream::from(quote! { #input })
}

// =========================================================================
//  MACRO: #[handler]
//  The Server-Side Magic.
//  1. Generates a Protocol Enum (`MyServiceProtocol`).
//  2. Generates `handle_cell_message` dispatcher.
//  3. Implements Zero-Copy Logic:
//     - Deserializes inputs from the raw request slice.
//     - Calls the user logic.
//     - Serializes output DIRECTLY into the ResponseSlot (Ring Buffer).
// =========================================================================
#[proc_macro_attribute]
pub fn handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    
    // Extract the struct name (e.g., "RendererService")
    let service_name = match &*input.self_ty {
        Type::Path(tp) => tp.path.segments.last().unwrap().ident.clone(),
        _ => panic!("Handler must implement a struct"),
    };

    let mut methods = Vec::new();
    
    // Iterate over methods in the impl block
    for item in &input.items {
        if let syn::ImplItem::Fn(method) = item {
            let name = method.sig.ident.clone();
            let args: Vec<_> = method
                .sig
                .inputs
                .iter()
                .filter_map(|arg| {
                    if let syn::FnArg::Typed(pat) = arg {
                        if let syn::Pat::Ident(id) = &*pat.pat {
                            return Some((id.ident.clone(), pat.ty.clone()));
                        }
                    }
                    None
                })
                .collect();
            methods.push((name, args));
        }
    }

    // 1. Generate Protocol Enum Variants
    let protocol_name = format_ident!("{}Protocol", service_name);
    let protocol_variants = methods.iter().map(|(name, args)| {
        let variant_name = format_ident!("{}", to_pascal_case(&name.to_string()));
        let fields = args.iter().map(|(n, t)| quote! { #n: #t });
        quote! { #variant_name { #(#fields),* } }
    });

    // 2. Determine Archived Protocol Name (rkyv convention)
    let archived_proto_name = format_ident!("Archived{}", protocol_name);

    // 3. Generate Dispatch Logic (Match Arms)
    let dispatch_arms = methods.iter().map(|(name, args)| {
        let variant_name = format_ident!("{}", to_pascal_case(&name.to_string()));
        let field_names: Vec<_> = args.iter().map(|(n, _)| n).collect();
        
        let method_calls = field_names.iter().map(|n| {
            // ARGUMENT DESERIALIZATION
            // We have `&Archived<T>`. We need `T` to call the user function.
            // This copies from SHM to Stack/Heap. 
            // NOTE: Ideally user fns would accept &Archived<T>, but that requires 
            // changing the user API. For now, we deserialize inputs.
            quote! {
                match ::cell_sdk::rkyv::Deserialize::deserialize(&#n, &mut ::cell_sdk::rkyv::Infallible) {
                    Ok(v) => v,
                    Err(_) => return Err(::anyhow::anyhow!("Arg deserialization failed")),
                }
            }
        });
        
        quote! {
            // Match against the Archived variant
            #archived_proto_name::#variant_name { #(#field_names),* } => {
                // Call the actual method
                let res = self.#name(#(#method_calls),*).await
                    .map_err(|e| e.to_string());
                
                // ZERO-COPY WRITE
                // We serialize the result directly into the slot (Ring Buffer).
                slot.serialize(&res)
                    .map_err(|e| ::anyhow::anyhow!("Serialization error: {}", e))?;
            }
        }
    });

    // 4. Schema Generation (Reflection)
    let schema_methods: Vec<MethodSchema> = methods
        .iter()
        .map(|(name, args)| {
            let inputs = args
                .iter()
                .map(|(n, t)| (n.to_string(), map_syn_type_to_ref(t)))
                .collect();
            MethodSchema {
                name: name.to_string(),
                inputs,
                output: TypeRef::Unknown, 
            }
        })
        .collect();

    let genome_struct = CellGenome {
        name: service_name.to_string(),
        fingerprint: 0x12345678, // TODO: Implement AST hashing
        methods: schema_methods,
        types: vec![],
    };
    let genome_json = serde_json::to_string(&genome_struct).unwrap();

    let expanded = quote! {
        #input

        // The Protocol Enum used for serialization
        #[derive(
            ::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize,
            ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize
        )]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(check_bytes)]
        #[archive(crate = "::cell_sdk::rkyv")]
        pub enum #protocol_name {
            #(#protocol_variants),*
        }

        impl #service_name {
            // Embed genome for runtime discovery
            pub const CELL_GENOME: &'static str = #genome_json;
            // Embed fingerprint for compile-time safety checks
            pub const SCHEMA_FINGERPRINT: u64 = 0x12345678;

            /// The main entry point for the Membrane to call.
            /// 
            /// Arguments:
            /// - `data`: The raw zero-copy slice containing the request.
            /// - `slot`: The output slot to write the response to (Zero-Copy).
            pub async fn handle_cell_message(
                &self, 
                data: &[u8], 
                slot: &mut ::cell_sdk::shm::ResponseSlot<'_>
            ) -> ::anyhow::Result<()> {
                
                // Zero-Copy Validation (No Allocation)
                let archived = ::cell_sdk::rkyv::check_archived_root::<#protocol_name>(data)
                    .map_err(|e| ::anyhow::anyhow!("Protocol Mismatch: {:?}", e))?;

                match archived {
                    #(#dispatch_arms),*
                }
                
                Ok(())
            }
        }
    };

    TokenStream::from(expanded)
}

// =========================================================================
//  MACRO: cell_remote!
//  The Client-Side Magic.
//  Usage: cell_remote!(MyClient = "exchange");
// =========================================================================

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

    // Strategy: Source -> Runtime -> Cache
    let (genome, source_path) = match try_source_analysis(&address_str) {
        Ok((g, p)) => (g, Some(p)),
        Err(_) => match try_runtime_connection(&address_str) {
            Ok(g) => (g, None),
            Err(_) => (try_cached_schema(&address_str).expect("Failed to resolve cell schema."), None),
        },
    };

    let client_code = generate_client(&genome, &name, &address_str);
    
    // Dependency Injection: Force Cargo to recompile if server source changes
    let dependency_hack = if let Some(path) = source_path {
        let path_str = path.to_str().unwrap();
        quote! { const _: &[u8] = include_bytes!(#path_str); }
    } else {
        quote! {}
    };

    TokenStream::from(quote! {
        #dependency_hack
        #client_code
    })
}

// --- Source Analysis Logic ---

fn try_source_analysis(cell_name: &str) -> Result<(CellGenome, PathBuf), String> {
    let paths = vec![
        PathBuf::from(format!("cells/{}/src/main.rs", cell_name)),
        PathBuf::from(format!("../{}/src/main.rs", cell_name)),
        dirs::home_dir().unwrap().join(format!(".cell/dna/{}/src/main.rs", cell_name)),
    ];

    for path in paths {
        if let Ok(abs_path) = std::fs::canonicalize(&path) {
            if abs_path.exists() {
                return Ok((parse_source_file(&abs_path)?, abs_path));
            }
        }
    }
    Err("Source not found".into())
}

fn parse_source_file(path: &Path) -> Result<CellGenome, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let syntax = syn::parse_file(&content).map_err(|e| e.to_string())?;

    let mut service_name = None;
    let mut methods = Vec::new();
    let mut types_found = Vec::new();

    // Pass 1: Find Service & Proteins
    for item in &syntax.items {
        if let syn::Item::Struct(s) = item {
            if has_attr(&s.attrs, "service") {
                service_name = Some(s.ident.to_string());
            }
            if has_attr(&s.attrs, "protein") {
                types_found.push(struct_to_type_schema(s));
            }
        }
        if let syn::Item::Enum(e) = item {
            if has_attr(&e.attrs, "protein") {
                types_found.push(enum_to_type_schema(e));
            }
        }
    }

    let service_name = service_name.ok_or("No #[cell::service] found in source")?;

    // Pass 2: Find Handler methods
    for item in &syntax.items {
        if let syn::Item::Impl(impl_block) = item {
            if has_attr(&impl_block.attrs, "handler") {
                if let Type::Path(tp) = &*impl_block.self_ty {
                    if tp.path.segments.last().unwrap().ident == service_name {
                        for item in &impl_block.items {
                            if let syn::ImplItem::Fn(m) = item {
                                methods.push(extract_method_schema(m));
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(CellGenome {
        name: service_name,
        fingerprint: 0,
        methods,
        types: types_found,
    })
}

fn extract_method_schema(m: &syn::ImplItemFn) -> MethodSchema {
    let inputs = m
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let syn::FnArg::Typed(pat) = arg {
                if let syn::Pat::Ident(id) = &*pat.pat {
                    return Some((id.ident.to_string(), map_syn_type_to_ref(&pat.ty)));
                }
            }
            None
        })
        .collect();

    let output = match &m.sig.output {
        syn::ReturnType::Default => TypeRef::Unit,
        syn::ReturnType::Type(_, ty) => map_syn_type_to_ref(ty),
    };

    MethodSchema {
        name: m.sig.ident.to_string(),
        inputs,
        output,
    }
}

// --- Runtime Connection Logic ---

fn try_runtime_connection(cell_name: &str) -> Result<CellGenome, String> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let home = dirs::home_dir().ok_or("No home dir")?;
        let socket_path = home.join(".cell/run").join(format!("{}.sock", cell_name));

        if !socket_path.exists() {
            // Mitosis Attempt
            let umbilical = home.join(".cell/run/mitosis.sock");
            if umbilical.exists() {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                if let Ok(mut stream) = tokio::net::UnixStream::connect(&umbilical).await {
                    let req = MitosisRequest::Spawn { cell_name: cell_name.to_string() };
                    let bytes = ::rkyv::to_bytes::<_, 256>(&req).unwrap().into_vec();
                    stream.write_all(&(bytes.len() as u32).to_le_bytes()).await.ok();
                    stream.write_all(&bytes).await.ok();
                    
                    let mut len_buf = [0u8; 4];
                    if stream.read_exact(&mut len_buf).await.is_ok() {
                        let len = u32::from_le_bytes(len_buf) as usize;
                        let mut buf = vec![0u8; len];
                        let _ = stream.read_exact(&mut buf).await;
                    }
                }
                for _ in 0..20 {
                    if socket_path.exists() { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }

        let mut stream = tokio::net::UnixStream::connect(&socket_path).await.map_err(|e| e.to_string())?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let req = b"__CELL_GENOME_REQUEST__";
        stream.write_all(&(req.len() as u32).to_le_bytes()).await.map_err(|e| e.to_string())?;
        stream.write_all(req).await.map_err(|e| e.to_string())?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await.map_err(|e| e.to_string())?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).await.map_err(|e| e.to_string())?;

        let g: CellGenome = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
        cache_genome(cell_name, &g).ok();
        Ok(g)
    })
}

fn try_cached_schema(cell_name: &str) -> Result<CellGenome, String> {
    let path = dirs::home_dir().unwrap().join(".cell/schemas").join(format!("{}.json", cell_name));
    let s = fs::read_to_string(path).map_err(|_| "No cache".to_string())?;
    serde_json::from_str(&s).map_err(|e| e.to_string())
}

fn cache_genome(cell_name: &str, g: &CellGenome) -> Result<(), String> {
    let dir = dirs::home_dir().unwrap().join(".cell/schemas");
    fs::create_dir_all(&dir).ok();
    let s = serde_json::to_string(g).unwrap();
    fs::write(dir.join(format!("{}.json", cell_name)), s).map_err(|e| e.to_string())
}

// --- Code Generation ---

fn generate_client(genome: &CellGenome, client_name: &syn::Ident, address: &str) -> proc_macro2::TokenStream {
    let protocol_enum_name = format_ident!("{}Protocol", genome.name);
    
    // 1. Enum Variants
    let variants = genome.methods.iter().map(|m| {
        let vname = format_ident!("{}", to_pascal_case(&m.name));
        let fields = m.inputs.iter().map(|(n, t)| {
            let fname = format_ident!("{}", n);
            let fty = type_ref_to_tokens(t);
            quote! { #fname: #fty }
        });
        quote! { #vname { #(#fields),* } }
    });

    let protocol_def = quote! {
        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize)]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(check_bytes)]
        #[archive(crate = "::cell_sdk::rkyv")]
        pub enum #protocol_enum_name { #(#variants),* }
    };

    let type_defs = genome.types.iter().map(|t| type_schema_to_tokens(t));

    // 2. Client Methods
    let method_impls = genome.methods.iter().map(|m| {
        let fn_name = format_ident!("{}", m.name);
        let variant_name = format_ident!("{}", to_pascal_case(&m.name));
        
        let args = m.inputs.iter().map(|(n, t)| {
            let id = format_ident!("{}", n);
            let ty = type_ref_to_tokens(t);
            quote! { #id: #ty }
        });
        
        let field_inits = m.inputs.iter().map(|(n, _)| {
            let id = format_ident!("{}", n);
            quote! { #id }
        });
        
        let return_ty = match &m.output {
            TypeRef::Result(ok, _) => type_ref_to_tokens(ok),
            TypeRef::Unit => quote! { () },
            t => type_ref_to_tokens(t),
        };

        quote! {
            pub async fn #fn_name(&mut self, #(#args),*) -> ::anyhow::Result<#return_ty> {
                let req = #protocol_enum_name::#variant_name { #(#field_inits),* };
                
                // Serialize request
                let bytes = ::cell_sdk::rkyv::to_bytes::<_, 1024>(&req)?.into_vec();
                
                // Send via Synapse
                let resp_vesicle = self.conn.fire_bytes(bytes).await?;
                
                // Deserialize Result<T, String>
                let res = ::cell_sdk::rkyv::from_bytes::<Result<#return_ty, String>>(resp_vesicle.as_slice())
                    .map_err(|e| ::anyhow::anyhow!("Deserialization error: {}", e))?;
                
                res.map_err(|e| ::anyhow::anyhow!(e))
            }
        }
    });

    quote! {
        #(#type_defs)*
        #protocol_def
        pub struct #client_name { conn: ::cell_sdk::Synapse }
        impl #client_name {
            pub async fn connect() -> ::anyhow::Result<Self> {
                let conn = ::cell_sdk::Synapse::grow(#address).await?;
                Ok(Self { conn })
            }
            pub fn address(&self) -> String { #address.to_string() }
            #(#method_impls)*
        }
    }
}

// --- AST Helpers ---

fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|a| {
        if a.path().is_ident(name) { return true; }
        if a.path().segments.len() == 2 {
            let segs: Vec<_> = a.path().segments.iter().collect();
            if segs[1].ident == name && (segs[0].ident == "cell" || segs[0].ident == "cell_sdk") { return true; }
        }
        false
    })
}

fn to_pascal_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn map_syn_type_to_ref(ty: &Type) -> TypeRef {
    match ty {
        Type::Path(tp) => {
            let seg = tp.path.segments.last().unwrap();
            let id = seg.ident.to_string();
            match id.as_str() {
                "String" => TypeRef::Primitive(Primitive::String),
                "u8" => TypeRef::Primitive(Primitive::U8),
                "u16" => TypeRef::Primitive(Primitive::U16),
                "u32" => TypeRef::Primitive(Primitive::U32),
                "u64" => TypeRef::Primitive(Primitive::U64),
                "f32" => TypeRef::Primitive(Primitive::F32),
                "bool" => TypeRef::Primitive(Primitive::Bool),
                "Vec" => match &seg.arguments {
                    syn::PathArguments::AngleBracketed(args) => {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            TypeRef::Vec(Box::new(map_syn_type_to_ref(inner)))
                        } else { TypeRef::Named("Vec".into()) }
                    }
                    _ => TypeRef::Named("Vec".into()),
                },
                "Option" => match &seg.arguments {
                    syn::PathArguments::AngleBracketed(args) => {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            TypeRef::Option(Box::new(map_syn_type_to_ref(inner)))
                        } else { TypeRef::Named("Option".into()) }
                    }
                    _ => TypeRef::Named("Option".into()),
                },
                "Result" => match &seg.arguments {
                    syn::PathArguments::AngleBracketed(args) => {
                        if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                            TypeRef::Result(Box::new(map_syn_type_to_ref(inner)), Box::new(TypeRef::Unit))
                        } else { TypeRef::Named("Result".into()) }
                    }
                    _ => TypeRef::Named("Result".into()),
                },
                _ => TypeRef::Named(id),
            }
        }
        Type::Array(_) => TypeRef::Named("Array".into()),
        _ => TypeRef::Unknown,
    }
}

fn type_ref_to_tokens(t: &TypeRef) -> proc_macro2::TokenStream {
    match t {
        TypeRef::Primitive(p) => match p {
            Primitive::String => quote! {String},
            Primitive::U8 => quote! {u8},
            Primitive::U16 => quote! {u16},
            Primitive::U32 => quote! {u32},
            Primitive::U64 => quote! {u64},
            Primitive::F32 => quote! {f32},
            Primitive::Bool => quote! {bool},
            _ => quote! {u64},
        },
        TypeRef::Named(s) => {
            if s == "Array" { quote! {[f32; 2]} } else {
                let i = format_ident!("{}", s);
                quote! {#i}
            }
        }
        TypeRef::Vec(inner) => {
            let i = type_ref_to_tokens(inner);
            quote! {Vec<#i>}
        }
        TypeRef::Option(inner) => {
            let i = type_ref_to_tokens(inner);
            quote! {Option<#i>}
        }
        TypeRef::Result(ok, _) => type_ref_to_tokens(ok),
        TypeRef::Unit => quote! {()},
        _ => quote! {()},
    }
}

fn struct_to_type_schema(s: &syn::ItemStruct) -> TypeSchema {
    let fields = match &s.fields {
        syn::Fields::Named(f) => f.named.iter()
            .map(|x| (x.ident.as_ref().unwrap().to_string(), map_syn_type_to_ref(&x.ty)))
            .collect(),
        _ => vec![],
    };
    TypeSchema { name: s.ident.to_string(), kind: TypeKind::Struct { fields } }
}

fn enum_to_type_schema(e: &syn::ItemEnum) -> TypeSchema {
    let variants = e.variants.iter()
        .map(|v| (v.ident.to_string(), vec![]))
        .collect();
    TypeSchema { name: e.ident.to_string(), kind: TypeKind::Enum { variants } }
}

fn type_schema_to_tokens(t: &TypeSchema) -> proc_macro2::TokenStream {
    let name = format_ident!("{}", t.name);
    match &t.kind {
        TypeKind::Struct { fields } => {
            let fs = fields.iter().map(|(n, tr)| {
                let ni = format_ident!("{}", n);
                let ti = type_ref_to_tokens(tr);
                quote! { pub #ni: #ti }
            });
            quote! {
                #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Clone, Debug, PartialEq)]
                #[serde(crate = "::cell_sdk::serde")]
                #[archive(check_bytes)]
                #[archive(crate = "::cell_sdk::rkyv")]
                pub struct #name { #(#fs),* }
            }
        }
        TypeKind::Enum { variants } => {
            let vs = variants.iter().map(|(n, _)| {
                let ni = format_ident!("{}", n);
                quote! { #ni }
            });
            quote! {
               #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Clone, Debug, Copy, PartialEq, Eq)]
               #[serde(crate = "::cell_sdk::serde")]
               #[archive(check_bytes)]
               #[archive(crate = "::cell_sdk::rkyv")]
               #[repr(u16)]
               pub enum #name { #(#vs),* }
            }
        }
    }
}