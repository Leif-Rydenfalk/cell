//! cell-macros  –  Procedural macros for cell-sdk
//! 0.1.2  (runtime-schema edition)

extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parse_macro_input, Expr, Field, Ident, Token, Type};

// ----------------------------------------------------------
// service_schema!  –  generates Request/Response types  +  JSON consts
// ----------------------------------------------------------
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

// ----------------------------------------------------------
// call_as!  –  type-safe client call  (runtime schema fallback)
// ----------------------------------------------------------
#[proc_macro]
pub fn call_as(input: TokenStream) -> TokenStream {
    let CallArgs { service, request } = parse_macro_input!(input as CallArgs);
    let service_lit = service.to_string();

    let expanded = quote! {{
        // 1.  obtain schema JSON  (build-time cache || runtime fetch)
        let schema_json = {
            if let Ok(out) = ::std::env::var("OUT_DIR") {
                let p = ::std::path::Path::new(&out)
                    .join(format!("{}_schema.json", #service_lit));
                if p.exists() {
                    ::std::fs::read_to_string(&p)
                        .expect("cannot read cached schema")
                } else {
                    ::cell_sdk::fetch_schema_runtime(#service_lit)
                        .expect("service not running & no cached schema")
                }
            } else {
                ::cell_sdk::fetch_schema_runtime(#service_lit)
                    .expect("service not running & no cached schema")
            }
        };

        // 2.  build request/response types from schema
        let (req_struct, req_type, resp_struct, resp_type) =
            ::cell_sdk::build::generate_structs_from_schema(&schema_json);

        // 3.  emit client code
        {
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

                // length-prefixed write
                stream.write_all(&(json.len() as u32).to_be_bytes())
                    .map_err(|e| ::anyhow::anyhow!("write length error: {}", e))?;
                stream.write_all(&json)
                    .map_err(|e| ::anyhow::anyhow!("write error: {}", e))?;
                stream.flush()
                    .map_err(|e| ::anyhow::anyhow!("flush error: {}", e))?;

                // length-prefixed read
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
        }
    }};

    TokenStream::from(expanded)
}

// ----------------------------------------------------------
// helpers
// ----------------------------------------------------------
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

// Re-export for the macro
pub use blake3;
