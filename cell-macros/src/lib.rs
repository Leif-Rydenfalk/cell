//! cell-macros – Procedural macros for cell-sdk

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use std::fs;
use std::path::PathBuf;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parse_macro_input, Expr, Field, Ident, Token, Type};

#[proc_macro]
pub fn service_schema(input: TokenStream) -> TokenStream {
    let schema = parse_macro_input!(input as ServiceSchema);
    let req_name = &schema.request_name;
    let req_fields = &schema.request_fields;
    let resp_name = &schema.response_name;
    let resp_fields = &schema.response_fields;

    let schema_json = generate_schema_json(&schema);
    let schema_hash = blake3::hash(schema_json.as_bytes()).to_hex().to_string();

    let expanded = quote! {
        #[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
        pub struct #req_name {
            #(pub #req_fields),*
        }
        #[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
        pub struct #resp_name {
            #(pub #resp_fields),*
        }
        #[doc(hidden)]
        pub const __CELL_SCHEMA__: &str = #schema_json;
        #[doc(hidden)]
        pub const __CELL_SCHEMA_HASH__: &str = #schema_hash;
    };
    TokenStream::from(expanded)
}

#[proc_macro]
pub fn call_as(input: TokenStream) -> TokenStream {
    let CallArgs { service, request } = parse_macro_input!(input as CallArgs);
    let service_lit = service.to_string();

    // 1. Locate Schema at Compile Time
    // The schema must exist in .cell-schemas/<service>.json (fetched by CLI)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR environment variable not set");

    let schema_path = PathBuf::from(&manifest_dir)
        .join(".cell-schemas")
        .join(format!("{}.json", service_lit));

    let schema_json = match fs::read_to_string(&schema_path) {
        Ok(content) => content,
        Err(_) => {
            let err_msg = format!(
                "Missing schema snapshot for service '{}'.\n\
                 Path checked: {}\n\
                 \n\
                 To fix this:\n\
                 1. Ensure dependencies are listed in cell.toml.\n\
                 2. Ensure dependencies are RUNNING (cell run <dep>).\n\
                 3. Run 'cell run .' in this directory to fetch snapshots.",
                service_lit,
                schema_path.display()
            );
            return TokenStream::from(quote! {
                compile_error!(#err_msg)
            });
        }
    };

    // 2. Generate Types & RPC Client Logic
    match parse_and_generate_types(&schema_json) {
        Ok((req_struct, resp_struct, req_type, resp_type)) => {
            let expanded = quote! {{
                // Embed types locally within this block scope
                #req_struct
                #resp_struct

                // Anonymous closure for the RPC call
                (|| -> ::anyhow::Result<#resp_type> {
                    use ::std::io::{Read, Write};

                    // --- DYNAMIC PATH RESOLUTION ---
                    // 1. Check Env Var (Injected by 'cell run')
                    // 2. Fallback to standard relative path (Sibling convention)

                    let env_key = format!("CELL_DEP_{}_SOCK", #service_lit.to_uppercase());
                    let sock_path = match ::std::env::var(&env_key) {
                        Ok(p) => p,
                        Err(_) => {
                            // Fallback assumes: ../<service_name>/run/cell.sock
                            format!("../{}/run/cell.sock", #service_lit)
                        }
                    };

                    // --- CONNECT ---
                    let mut stream = ::std::os::unix::net::UnixStream::connect(&sock_path)
                        .map_err(|e| ::anyhow::anyhow!(
                            "Failed to connect to service '{}' at '{}'. Is it running? Error: {}",
                            #service_lit, sock_path, e
                        ))?;

                    // --- SERIALIZE ---
                    let request: #req_type = #request;
                    let json_req = ::serde_json::to_vec(&request)
                        .map_err(|e| ::anyhow::anyhow!("Serialization failed: {}", e))?;

                    // --- SEND ---
                    stream.write_all(&(json_req.len() as u32).to_be_bytes())?;
                    stream.write_all(&json_req)?;
                    stream.flush()?;

                    // --- RECEIVE ---
                    let mut len_buf = [0u8; 4];
                    stream.read_exact(&mut len_buf)
                        .map_err(|e| ::anyhow::anyhow!("Failed to read response length: {}", e))?;

                    let len = u32::from_be_bytes(len_buf) as usize;

                    // Sanity check (16MB)
                    if len > 16 * 1024 * 1024 {
                         return Err(::anyhow::anyhow!("Response too large: {} bytes", len));
                    }

                    let mut resp_buf = vec![0u8; len];
                    stream.read_exact(&mut resp_buf)
                        .map_err(|e| ::anyhow::anyhow!("Failed to read response body: {}", e))?;

                    // --- DESERIALIZE ---
                    ::serde_json::from_slice(&resp_buf)
                        .map_err(|e| ::anyhow::anyhow!("Deserialization failed: {}", e))
                })()
            }};
            TokenStream::from(expanded)
        }
        Err(e) => {
            let err = format!("Schema parsing failed for '{}': {}", service_lit, e);
            TokenStream::from(quote! { compile_error!(#err) })
        }
    }
}

// ---------- PARSING & GENERATION HELPERS ----------

fn parse_and_generate_types(
    schema_json: &str,
) -> Result<
    (
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
    ),
    String,
> {
    let schema: serde_json::Value =
        serde_json::from_str(schema_json).map_err(|e| format!("invalid JSON: {}", e))?;

    let req_name = schema["request"]["name"]
        .as_str()
        .ok_or("missing req name")?;
    let resp_name = schema["response"]["name"]
        .as_str()
        .ok_or("missing resp name")?;

    let req_ident = syn::Ident::new(req_name, proc_macro2::Span::call_site());
    let resp_ident = syn::Ident::new(resp_name, proc_macro2::Span::call_site());

    let req_fields = generate_fields(
        schema["request"]["fields"]
            .as_array()
            .ok_or("no req fields")?,
    )?;
    let resp_fields = generate_fields(
        schema["response"]["fields"]
            .as_array()
            .ok_or("no resp fields")?,
    )?;

    let req_struct = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[derive(::serde::Serialize, ::serde::Deserialize, Debug, Clone)]
        struct #req_ident { #(#req_fields),* }
    };

    let resp_struct = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[derive(::serde::Serialize, ::serde::Deserialize, Debug, Clone)]
        struct #resp_ident { #(#resp_fields),* }
    };

    Ok((
        req_struct,
        resp_struct,
        quote! {#req_ident},
        quote! {#resp_ident},
    ))
}

fn generate_fields(fields: &[serde_json::Value]) -> Result<Vec<proc_macro2::TokenStream>, String> {
    let mut out = Vec::new();
    for f in fields {
        let name = f["name"].as_str().ok_or("missing field name")?;
        let ty = f["type"].as_str().ok_or("missing field type")?;

        let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
        let ty_ident: syn::Type = syn::parse_str(ty).map_err(|e| format!("bad type {}", e))?;

        out.push(quote! { pub #ident: #ty_ident });
    }
    Ok(out)
}

// ---------- SYN PARSERS ----------

struct ServiceSchema {
    request_name: Ident,
    request_fields: Vec<Field>,
    response_name: Ident,
    response_fields: Vec<Field>,
}

impl Parse for ServiceSchema {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Syntax: service: Name, request: Name { ... }, response: Name { ... }
        input.parse::<Ident>()?; // "service"
        input.parse::<Token![:]>()?;
        let _ = input.parse::<Ident>()?; // skip name
        input.parse::<Token![,]>()?;

        input.parse::<Ident>()?; // "request"
        input.parse::<Token![:]>()?;
        let request_name = input.parse()?;
        let content;
        braced!(content in input);
        let request_fields = parse_fields(&content)?;
        input.parse::<Token![,]>()?;

        input.parse::<Ident>()?; // "response"
        input.parse::<Token![:]>()?;
        let response_name = input.parse()?;
        let content;
        braced!(content in input);
        let response_fields = parse_fields(&content)?;

        Ok(ServiceSchema {
            request_name,
            request_fields,
            response_name,
            response_fields,
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

struct CallArgs {
    service: Ident,
    request: Expr,
}

impl Parse for CallArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let service = input.parse()?;
        input.parse::<Token![,]>()?;
        let request = input.parse()?;
        Ok(CallArgs { service, request })
    }
}

fn generate_schema_json(schema: &ServiceSchema) -> String {
    let jsonify = |fields: &[Field]| {
        fields
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.ident.as_ref().unwrap().to_string(),
                    "type": quote::quote!(#f.ty).to_string()
                })
            })
            .collect::<Vec<_>>()
    };

    serde_json::json!({
        "request": {
            "name": schema.request_name.to_string(),
            "fields": jsonify(&schema.request_fields)
        },
        "response": {
            "name": schema.response_name.to_string(),
            "fields": jsonify(&schema.response_fields)
        }
    })
    .to_string()
}
