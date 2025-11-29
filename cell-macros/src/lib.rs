extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use syn::parse::Parser;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parse_macro_input, Data, DeriveInput, Expr, Field, Ident, LitStr, Token, Type};

// --- SHARED SCHEMA STRUCTS ---
#[derive(Serialize, Deserialize, Debug, Clone)]
struct CellGenome {
    name: String,
    fingerprint: u64,
    methods: Vec<MethodSchema>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
struct MethodSchema {
    name: String,
    inputs: Vec<(String, TypeRef)>,
    output: TypeRef,
    is_stream: bool,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
enum TypeRef {
    Named(String),
    Primitive(Primitive),
    Vec(Box<TypeRef>),
    Option(Box<TypeRef>),
    Unknown,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
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

// -------------------------------------------------------------------------
// 1. #[protein]
// -------------------------------------------------------------------------

struct ProteinArgs {
    class: Option<String>,
}
impl ProteinArgs {
    fn parse(attr: TokenStream) -> Self {
        let mut class = None;
        if !attr.is_empty() {
            let parser = syn::meta::parser(|meta| {
                if meta.path.is_ident("class") {
                    let value: LitStr = meta.value()?.parse()?;
                    class = Some(value.value());
                    Ok(())
                } else {
                    Ok(())
                }
            });
            let _ = parser.parse(attr);
        }
        Self { class }
    }
}

#[proc_macro_attribute]
pub fn protein(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = ProteinArgs::parse(attr);
    let input = parse_macro_input!(item as DeriveInput);
    let struct_name = &input.ident;

    let ast_string = quote!(#input).to_string();
    let mut hasher = blake3::Hasher::new();
    hasher.update(ast_string.as_bytes());
    let hash_bytes = hasher.finalize();
    let fp_bytes: [u8; 8] = hash_bytes.as_bytes()[0..8].try_into().unwrap();
    let fp_u64 = u64::from_le_bytes(fp_bytes);

    let genome = extract_genome_from_derive(
        &input,
        fp_u64,
        args.class.as_deref().unwrap_or(&struct_name.to_string()),
    );
    let genome_json = serde_json::to_string(&genome).expect("Failed to serialize genome");

    let expanded = quote! {
        #[derive(
            ::cell_sdk::serde::Serialize,
            ::cell_sdk::serde::Deserialize,
            ::cell_sdk::rkyv::Archive,
            ::cell_sdk::rkyv::Serialize,
            ::cell_sdk::rkyv::Deserialize,
            Clone,
            Debug,
        )]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(crate = "::cell_sdk::rkyv")]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        #input

        impl #struct_name {
            pub const SCHEMA_FINGERPRINT: u64 = #fp_u64;
            pub const CELL_GENOME: &'static str = #genome_json;
        }
    };

    TokenStream::from(expanded)
}

fn extract_genome_from_derive(input: &DeriveInput, fp: u64, name: &str) -> CellGenome {
    let mut methods = Vec::new();
    if let Data::Enum(data) = &input.data {
        for variant in &data.variants {
            let method_name = variant.ident.to_string();
            let mut inputs = Vec::new();
            for field in &variant.fields {
                let fname = field
                    .ident
                    .as_ref()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "arg".to_string());
                let ftype = map_syn_type_to_ref(&field.ty);
                inputs.push((fname, ftype));
            }
            methods.push(MethodSchema {
                name: method_name,
                inputs,
                output: TypeRef::Unknown,
                is_stream: false,
            });
        }
    }
    CellGenome {
        name: name.to_string(),
        fingerprint: fp,
        methods,
    }
}

// -------------------------------------------------------------------------
// 2. cell_remote!
// -------------------------------------------------------------------------

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

    let genome = resolve_genome(&address_str)
        .unwrap_or_else(|e| panic!("Failed to resolve cell '{}': {}", address_str, e));

    let client_code = generate_client(&genome, &name, &address_str);
    TokenStream::from(client_code)
}

fn resolve_genome(cell_name: &str) -> Result<CellGenome, String> {
    if let Ok(genome) = try_source_analysis(cell_name) {
        return Ok(genome);
    }
    if let Ok(genome) = try_runtime_connection(cell_name) {
        let _ = cache_genome(cell_name, &genome);
        return Ok(genome);
    }
    if let Ok(genome) = try_cached_schema(cell_name) {
        return Ok(genome);
    }
    Err("Could not locate genome.".into())
}

// Source Analysis
fn try_source_analysis(cell_name: &str) -> Result<CellGenome, String> {
    let paths = vec![
        PathBuf::from(format!("../{}", cell_name)),
        PathBuf::from(format!("cells/{}", cell_name)),
        PathBuf::from(format!("../cells/{}", cell_name)),
        dirs::home_dir()
            .unwrap()
            .join(format!(".cell/dna/{}", cell_name)),
    ];
    for base in paths {
        let src_main = base.join("src/main.rs");
        if src_main.exists() {
            if let Ok(g) = parse_file_for_protein(&src_main) {
                return Ok(g);
            }
        }
    }
    Err("Source not found".into())
}

fn parse_file_for_protein(path: &Path) -> Result<CellGenome, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let syntax = syn::parse_file(&content).map_err(|e| e.to_string())?;
    for item in syntax.items {
        if let syn::Item::Enum(e) = item {
            if e.attrs.iter().any(|attr| attr.path().is_ident("protein")) {
                let name = e.ident.to_string();
                let fingerprint = 0xCAFEBABE;
                let mut methods = Vec::new();
                for variant in e.variants {
                    let m_name = variant.ident.to_string();
                    let mut inputs = Vec::new();
                    for field in variant.fields {
                        let fname = field
                            .ident
                            .as_ref()
                            .map(|i| i.to_string())
                            .unwrap_or_else(|| "arg".to_string());
                        let ftype = map_syn_type_to_ref(&field.ty);
                        inputs.push((fname, ftype));
                    }
                    methods.push(MethodSchema {
                        name: m_name,
                        inputs,
                        output: TypeRef::Unknown,
                        is_stream: false,
                    });
                }
                return Ok(CellGenome {
                    name,
                    fingerprint,
                    methods,
                });
            }
        }
    }
    Err("No #[protein] found".into())
}

// Runtime Connection
fn try_runtime_connection(cell_name: &str) -> Result<CellGenome, String> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let home = dirs::home_dir().ok_or("No home dir")?;
        let socket_path = home.join(".cell/run").join(format!("{}.sock", cell_name));

        let mut stream = tokio::net::UnixStream::connect(&socket_path)
            .await
            .map_err(|e| e.to_string())?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let req = b"__CELL_GENOME_REQUEST__";
        stream
            .write_all(&(req.len() as u32).to_le_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stream.write_all(req).await.map_err(|e| e.to_string())?;

        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| e.to_string())?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 {
            return Err("Empty genome".into());
        }
        let mut buf = vec![0u8; len];
        stream
            .read_exact(&mut buf)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_slice::<CellGenome>(&buf).map_err(|e| e.to_string())
    })
}

// Cache
fn try_cached_schema(cell_name: &str) -> Result<CellGenome, String> {
    let cache_path = dirs::home_dir()
        .unwrap()
        .join(".cell/schemas")
        .join(format!("{}.json", cell_name));
    if cache_path.exists() {
        let content = fs::read_to_string(&cache_path).map_err(|e| e.to_string())?;
        Ok(serde_json::from_str(&content).map_err(|e| e.to_string())?)
    } else {
        Err("No cache".into())
    }
}
fn cache_genome(cell_name: &str, genome: &CellGenome) -> Result<(), String> {
    let cache_dir = dirs::home_dir().unwrap().join(".cell/schemas");
    fs::create_dir_all(&cache_dir).ok();
    let json = serde_json::to_string_pretty(genome).map_err(|e| e.to_string())?;
    fs::write(cache_dir.join(format!("{}.json", cell_name)), json).map_err(|e| e.to_string())?;
    Ok(())
}

fn generate_client(
    genome: &CellGenome,
    client_name: &syn::Ident,
    address: &str,
) -> proc_macro2::TokenStream {
    let protocol_enum_name = format_ident!("{}", genome.name);
    let variants = genome.methods.iter().map(|m| {
        let vname = format_ident!("{}", m.name);
        let fields = m.inputs.iter().map(|(n, t)| {
            let fname = format_ident!("{}", n);
            let fty = type_ref_to_rust_token(t);
            quote! { #fname: #fty }
        });
        quote! { #vname { #(#fields),* } }
    });

    let protocol_def = quote! {
        #[derive(
            ::cell_sdk::serde::Serialize,
            ::cell_sdk::serde::Deserialize,
            ::cell_sdk::rkyv::Archive,
            ::cell_sdk::rkyv::Serialize,
            ::cell_sdk::rkyv::Deserialize,
            Clone, Debug
        )]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(crate = "::cell_sdk::rkyv")]
        #[archive(check_bytes)]
        pub enum #protocol_enum_name {
            #(#variants),*
        }
    };

    let method_impls = genome.methods.iter().map(|m| {
        let raw_name = &m.name;
        let fn_name = format_ident!("{}", to_snake_case(raw_name));
        let variant_name = format_ident!("{}", raw_name);
        let args = m.inputs.iter().map(|(n, t)| {
            let name = format_ident!("{}", n);
            let ty = type_ref_to_rust_token(t);
            quote! { #name: #ty }
        });
        let field_inits = m.inputs.iter().map(|(n, _)| {
            let name = format_ident!("{}", n);
            quote! { #name }
        });
        quote! {
            pub async fn #fn_name(&mut self, #(#args),*) -> ::anyhow::Result<::cell_sdk::vesicle::Vesicle> {
                let msg = #protocol_enum_name::#variant_name {
                    #(#field_inits),*
                };
                self.conn.fire(msg).await
            }
        }
    });

    // NOTE: Removed the wrapping block {} and trailing expression to allow top-level definition.
    quote! {
        #protocol_def

        pub struct #client_name {
            conn: ::cell_sdk::Synapse,
        }

        impl #client_name {
            pub async fn connect() -> ::anyhow::Result<Self> {
                let conn = ::cell_sdk::Synapse::grow(#address).await?;
                Ok(Self { conn })
            }

            pub fn address(&self) -> &'static str {
                #address
            }

            #(#method_impls)*
        }
    }
}

// Helpers
fn to_snake_case(s: &str) -> String {
    let mut acc = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i != 0 {
            acc.push('_');
        }
        acc.push(c.to_ascii_lowercase());
    }
    acc
}
fn map_syn_type_to_ref(ty: &Type) -> TypeRef {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            let s = seg.ident.to_string();
            return match s.as_str() {
                "String" => TypeRef::Primitive(Primitive::String),
                "u8" => TypeRef::Primitive(Primitive::U8),
                "u32" => TypeRef::Primitive(Primitive::U32),
                "u64" => TypeRef::Primitive(Primitive::U64),
                "bool" => TypeRef::Primitive(Primitive::Bool),
                "Vec" => TypeRef::Vec(Box::new(TypeRef::Unknown)),
                _ => TypeRef::Named(s),
            };
        }
    }
    TypeRef::Unknown
}
fn type_ref_to_rust_token(ty: &TypeRef) -> proc_macro2::TokenStream {
    match ty {
        TypeRef::Primitive(p) => match p {
            Primitive::String => quote! { String },
            Primitive::U64 => quote! { u64 },
            Primitive::U32 => quote! { u32 },
            Primitive::U8 => quote! { u8 },
            Primitive::Bool => quote! { bool },
            _ => quote! { u64 },
        },
        TypeRef::Vec(inner) => {
            let t = type_ref_to_rust_token(inner);
            quote! { Vec<#t> }
        }
        _ => quote! { String },
    }
}

// -------------------------------------------------------------------------
// 3. RESTORED: signal_receptor & call_as (Legacy)
// -------------------------------------------------------------------------

struct ReceptorDef {
    name: Ident,
    input_name: Ident,
    input_fields: Vec<Field>,
    output_name: Ident,
    output_fields: Vec<Field>,
}

impl Parse for ReceptorDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let _ = input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let name = input.parse()?;
        input.parse::<Token![,]>()?;
        let _ = input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let input_name = input.parse()?;
        let content;
        braced!(content in input);
        let input_fields = parse_fields(&content)?;
        input.parse::<Token![,]>()?;
        let _ = input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let output_name = input.parse()?;
        let content2;
        braced!(content2 in input);
        let output_fields = parse_fields(&content2)?;
        Ok(ReceptorDef {
            name,
            input_name,
            input_fields,
            output_name,
            output_fields,
        })
    }
}

fn parse_fields(content: ParseStream) -> syn::Result<Vec<Field>> {
    let mut fields = Vec::new();
    while !content.is_empty() {
        let name: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        let ty: Type = content.parse()?;
        fields.push(Field {
            attrs: vec![],
            vis: syn::Visibility::Inherited,
            mutability: syn::FieldMutability::None,
            ident: Some(name),
            colon_token: Some(Default::default()),
            ty,
        });
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
    }
    Ok(fields)
}

#[proc_macro]
pub fn signal_receptor(input: TokenStream) -> TokenStream {
    let schema = parse_macro_input!(input as ReceptorDef);
    let req_name = &schema.input_name;
    let req_fields = &schema.input_fields;
    let resp_name = &schema.output_name;
    let resp_fields = &schema.output_fields;
    let expanded = quote! {
        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Clone, Debug)]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(crate = "::cell_sdk::rkyv")]
        #[archive(check_bytes)]
        pub struct #req_name { #(pub #req_fields),* }
        #[derive(::cell_sdk::serde::Serialize, ::cell_sdk::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Clone, Debug)]
        #[archive(crate = "::cell_sdk::rkyv")]
        #[archive(check_bytes)]
        pub struct #resp_name { #(pub #resp_fields),* }
        #[doc(hidden)]
        pub const __GENOME__: &str = "";
    };
    TokenStream::from(expanded)
}

struct CallArgs {
    cell_name: Ident,
    signal_data: Expr,
}
impl Parse for CallArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let cell_name = input.parse()?;
        input.parse::<Token![,]>()?;
        let signal_data = input.parse()?;
        Ok(CallArgs {
            cell_name,
            signal_data,
        })
    }
}

#[proc_macro]
pub fn call_as(input: TokenStream) -> TokenStream {
    let CallArgs { cell_name, .. } = parse_macro_input!(input as CallArgs);
    let error = format!(
        "call_as! for {} is deprecated in Cell v0.3. Use cell_remote!.",
        cell_name
    );
    TokenStream::from(quote! { compile_error!(#error) })
}
