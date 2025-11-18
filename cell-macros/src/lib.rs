//! cell-macros – Procedural macros for cell-sdk (SQLx-style snapshots)

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
    let schema_hash = compute_hash(&schema_json);

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

    // Read schema snapshot from caller's .cell-schemas directory
    let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => dir,
        Err(_) => {
            let err_msg = "CARGO_MANIFEST_DIR not set";
            return TokenStream::from(quote! {
                compile_error!(#err_msg)
            });
        }
    };

    let schema_dir = PathBuf::from(&manifest_dir).join(".cell-schemas");
    let schema_path = schema_dir.join(format!("{}.json", service_lit));
    let hash_path = schema_dir.join(format!("{}.hash", service_lit));

    // Try to read snapshot
    let schema_json = match fs::read_to_string(&schema_path) {
        Ok(content) => content,
        Err(_) => {
            let err_msg = format!(
                "Missing schema snapshot for service '{}'.\n\
                 \n\
                 The snapshot should be in: .cell-schemas/{}.json\n\
                 \n\
                 To fix this:\n\
                 1. Start the dependency: cell run <path-to-{}>\n\
                 2. Build this cell: cell run .\n\
                    (The CLI will auto-snapshot dependencies)\n\
                 \n\
                 Or if the service is already running:\n\
                 - Ensure cell.toml has: [deps]\n\
                 - Ensure it lists: {} = \"0.1\"\n\
                 - Run: cell run .\n\
                 \n\
                 The snapshot enables offline/hermetic builds (like SQLx).",
                service_lit, service_lit, service_lit, service_lit
            );
            return TokenStream::from(quote! {
                compile_error!(#err_msg)
            });
        }
    };

    // Verify hash for drift detection
    let current_hash = compute_hash(&schema_json);
    match fs::read_to_string(&hash_path) {
        Ok(saved_hash) => {
            if saved_hash.trim() != current_hash {
                let err_msg = format!(
                    "Schema drift detected for service '{}'!\n\
                     \n\
                     Expected hash: {}\n\
                     Actual hash:   {}\n\
                     \n\
                     The cached schema doesn't match the service's current schema.\n\
                     This means the service definition changed but the snapshot is stale.\n\
                     \n\
                     To fix:\n\
                     1. Start the updated service: cell run <path-to-{}>\n\
                     2. Rebuild this cell: cell run .\n\
                     \n\
                     The CLI will automatically refresh the snapshot.",
                    service_lit,
                    saved_hash.trim(),
                    current_hash,
                    service_lit
                );
                return TokenStream::from(quote! {
                    compile_error!(#err_msg)
                });
            }
        }
        Err(_) => {
            // Hash file missing - create it (first time)
            let _ = fs::write(&hash_path, &current_hash);
        }
    }

    // Generate types from schema
    match parse_and_generate_types(&schema_json) {
        Ok((req_struct, resp_struct, req_type, resp_type)) => {
            let expanded = quote! {{
                #req_struct
                #resp_struct

                (|| -> ::anyhow::Result<#resp_type> {
                    use ::std::io::{Read, Write};
                    let sock_path = format!("/tmp/cell/sockets/{}.sock", #service_lit);
                    let mut stream = ::std::os::unix::net::UnixStream::connect(&sock_path)
                        .map_err(|e| ::anyhow::anyhow!("cannot connect to {}: {}", #service_lit, e))?;

                    let request: #req_type = #request;
                    let json = ::serde_json::to_vec(&request)
                        .map_err(|e| ::anyhow::anyhow!("serialize error: {}", e))?;

                    stream.write_all(&(json.len() as u32).to_be_bytes())
                        .map_err(|e| ::anyhow::anyhow!("write length error: {}", e))?;
                    stream.write_all(&json)
                        .map_err(|e| ::anyhow::anyhow!("write error: {}", e))?;
                    stream.flush()
                        .map_err(|e| ::anyhow::anyhow!("flush error: {}", e))?;

                    let mut len_buf = [0u8; 4];
                    stream.read_exact(&mut len_buf)
                        .map_err(|e| ::anyhow::anyhow!("read length error: {}", e))?;
                    let len = u32::from_be_bytes(len_buf) as usize;
                    let mut resp_buf = vec![0u8; len];
                    stream.read_exact(&mut resp_buf)
                        .map_err(|e| ::anyhow::anyhow!("read response error: {}", e))?;

                    ::serde_json::from_slice(&resp_buf)
                        .map_err(|e| ::anyhow::anyhow!("deserialize error: {}", e))
                })()
            }};
            TokenStream::from(expanded)
        }
        Err(e) => {
            let err_msg = format!("Failed to parse schema for '{}': {}", service_lit, e);
            TokenStream::from(quote! {
                compile_error!(#err_msg)
            })
        }
    }
}

// ---------- Type generation ----------

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

    let req_name_str = schema["request"]["name"]
        .as_str()
        .ok_or("missing request.name")?;
    let req_fields = schema["request"]["fields"]
        .as_array()
        .ok_or("missing request.fields")?;

    let resp_name_str = schema["response"]["name"]
        .as_str()
        .ok_or("missing response.name")?;
    let resp_fields = schema["response"]["fields"]
        .as_array()
        .ok_or("missing response.fields")?;

    let req_type_ident = syn::Ident::new(req_name_str, proc_macro2::Span::call_site());
    let resp_type_ident = syn::Ident::new(resp_name_str, proc_macro2::Span::call_site());

    let mut req_field_defs = Vec::new();
    for field in req_fields {
        let name = field["name"].as_str().ok_or("missing field name")?;
        let ty = field["type"].as_str().ok_or("missing field type")?;

        let field_ident = syn::Ident::new(name, proc_macro2::Span::call_site());
        let field_type: syn::Type =
            syn::parse_str(ty).map_err(|e| format!("invalid type '{}': {}", ty, e))?;

        req_field_defs.push(quote! {
            pub #field_ident: #field_type
        });
    }

    let mut resp_field_defs = Vec::new();
    for field in resp_fields {
        let name = field["name"].as_str().ok_or("missing field name")?;
        let ty = field["type"].as_str().ok_or("missing field type")?;

        let field_ident = syn::Ident::new(name, proc_macro2::Span::call_site());
        let field_type: syn::Type =
            syn::parse_str(ty).map_err(|e| format!("invalid type '{}': {}", ty, e))?;

        resp_field_defs.push(quote! {
            pub #field_ident: #field_type
        });
    }

    let req_struct = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[derive(::serde::Serialize, ::serde::Deserialize, Debug, Clone)]
        struct #req_type_ident {
            #(#req_field_defs),*
        }
    };

    let resp_struct = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[derive(::serde::Serialize, ::serde::Deserialize, Debug, Clone)]
        struct #resp_type_ident {
            #(#resp_field_defs),*
        }
    };

    let req_type = quote! { #req_type_ident };
    let resp_type = quote! { #resp_type_ident };

    Ok((req_struct, resp_struct, req_type, resp_type))
}

// ---------- Parser ----------

struct ServiceSchema {
    request_name: Ident,
    request_fields: Vec<Field>,
    response_name: Ident,
    response_fields: Vec<Field>,
}

impl Parse for ServiceSchema {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<Ident>()?; // "service"
        input.parse::<Token![:]>()?;
        let _service_name = input.parse::<Ident>()?;
        input.parse::<Token![,]>()?;

        input.parse::<Ident>()?; // "request"
        input.parse::<Token![:]>()?;
        let request_name = input.parse::<Ident>()?;
        let content;
        braced!(content in input);
        let mut request_fields = Vec::new();
        while !content.is_empty() {
            let field_name: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            let field_type: Type = content.parse()?;
            request_fields.push(Field {
                attrs: vec![],
                vis: syn::Visibility::Inherited,
                mutability: syn::FieldMutability::None,
                ident: Some(field_name),
                colon_token: Some(Default::default()),
                ty: field_type,
            });
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
        input.parse::<Token![,]>()?;

        input.parse::<Ident>()?; // "response"
        input.parse::<Token![:]>()?;
        let response_name = input.parse::<Ident>()?;
        let content;
        braced!(content in input);
        let mut response_fields = Vec::new();
        while !content.is_empty() {
            let field_name: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            let field_type: Type = content.parse()?;
            response_fields.push(Field {
                attrs: vec![],
                vis: syn::Visibility::Inherited,
                mutability: syn::FieldMutability::None,
                ident: Some(field_name),
                colon_token: Some(Default::default()),
                ty: field_type,
            });
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
        Ok(ServiceSchema {
            request_name,
            request_fields,
            response_name,
            response_fields,
        })
    }
}

struct CallArgs {
    service: Ident,
    request: Expr,
}

impl Parse for CallArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let service: Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let request: Expr = input.parse()?;
        Ok(CallArgs { service, request })
    }
}

fn generate_schema_json(schema: &ServiceSchema) -> String {
    let req_fields = schema
        .request_fields
        .iter()
        .map(|f| {
            let name = f.ident.as_ref().unwrap().to_string();
            let ty = quote::quote!(#f.ty).to_string();
            serde_json::json!({ "name": name, "type": ty })
        })
        .collect::<Vec<_>>();
    let resp_fields = schema
        .response_fields
        .iter()
        .map(|f| {
            let name = f.ident.as_ref().unwrap().to_string();
            let ty = quote::quote!(#f.ty).to_string();
            serde_json::json!({ "name": name, "type": ty })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "request":  { "name": schema.request_name.to_string(),  "fields": req_fields  },
        "response": { "name": schema.response_name.to_string(), "fields": resp_fields },
    })
    .to_string()
}

fn compute_hash(data: &str) -> String {
    blake3::hash(data.as_bytes()).to_hex().to_string()
}
