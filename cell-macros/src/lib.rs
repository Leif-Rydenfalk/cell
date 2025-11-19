extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
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
        #[derive(::serde::Serialize, ::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        pub struct #req_name { #(pub #req_fields),* }

        #[derive(::serde::Serialize, ::serde::Deserialize, ::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        pub struct #resp_name { #(pub #resp_fields),* }

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

    // We still look up schema at compile time to ensure type safety
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let schema_path = PathBuf::from(&manifest_dir)
        .join(".cell-schemas")
        .join(format!("{}.json", service_lit));

    let schema_json = match fs::read_to_string(&schema_path) {
        Ok(content) => content,
        Err(_) => {
            return TokenStream::from(
                quote! { compile_error!("Missing schema snapshot. Run 'cell run .' first.") },
            )
        }
    };

    match parse_and_generate_types(&schema_json) {
        Ok((req_struct, resp_struct, req_type, resp_type)) => {
            let expanded = quote! {{
                #req_struct
                #resp_struct

                (|| -> ::anyhow::Result<#resp_type> {
                    use ::cell_sdk::rkyv::Deserialize;
                    // New Logic: Just call invoke_rpc with the name. SDK handles routing.
                    let request: #req_type = #request;

                    let payload = ::cell_sdk::rkyv::to_bytes::<_, 1024>(&request)
                        .map_err(|e| ::anyhow::anyhow!("Rkyv serialize error: {}", e))?;

                    let response_bytes = ::cell_sdk::invoke_rpc(#service_lit, &payload)?;

                    let archived = ::cell_sdk::rkyv::check_archived_root::<#resp_type>(&response_bytes)
                        .map_err(|e| ::anyhow::anyhow!("Rkyv validation error: {}", e))?;

                    let deserialized: #resp_type = archived.deserialize(&mut ::cell_sdk::rkyv::Infallible)
                         .map_err(|e| ::anyhow::anyhow!("Rkyv deserialize error: {}", e))?;

                    Ok(deserialized)
                })()
            }};
            TokenStream::from(expanded)
        }
        Err(e) => TokenStream::from(quote! { compile_error!(#e) }),
    }
}

// --- Helpers (Same as before) ---

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
    let schema: serde_json::Value = serde_json::from_str(schema_json).map_err(|e| e.to_string())?;
    let req_name = schema["request"]["name"]
        .as_str()
        .ok_or("Missing req name")?;
    let resp_name = schema["response"]["name"]
        .as_str()
        .ok_or("Missing resp name")?;
    let req_ident = syn::Ident::new(req_name, proc_macro2::Span::call_site());
    let resp_ident = syn::Ident::new(resp_name, proc_macro2::Span::call_site());
    let req_fields = generate_fields(
        schema["request"]["fields"]
            .as_array()
            .ok_or("Missing req fields")?,
    )?;
    let resp_fields = generate_fields(
        schema["response"]["fields"]
            .as_array()
            .ok_or("Missing resp fields")?,
    )?;

    let req_struct = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
        #[archive(check_bytes)]
        struct #req_ident { #(#req_fields),* }
    };
    let resp_struct = quote! {
        #[allow(non_camel_case_types, dead_code)]
        #[derive(::cell_sdk::rkyv::Archive, ::cell_sdk::rkyv::Serialize, ::cell_sdk::rkyv::Deserialize, Debug, Clone)]
        #[archive(check_bytes)]
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
        let name = f["name"].as_str().unwrap();
        let ty_str = f["type"].as_str().unwrap();
        let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
        let ty_ident: syn::Type = syn::parse_str(ty_str).map_err(|e| e.to_string())?;
        out.push(quote! { pub #ident: #ty_ident });
    }
    Ok(out)
}

struct ServiceSchema {
    request_name: Ident,
    request_fields: Vec<Field>,
    response_name: Ident,
    response_fields: Vec<Field>,
}
impl Parse for ServiceSchema {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let _ = input.parse::<Ident>()?;
        input.parse::<Token![,]>()?;
        input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let request_name = input.parse()?;
        let content;
        braced!(content in input);
        let request_fields = parse_fields(&content)?;
        input.parse::<Token![,]>()?;
        input.parse::<Ident>()?;
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
        fields.iter().map(|f| {
        let mut ts = proc_macro2::TokenStream::new(); f.ty.to_tokens(&mut ts);
        serde_json::json!({ "name": f.ident.as_ref().unwrap().to_string(), "type": ts.to_string() })
    }).collect::<Vec<_>>()
    };
    serde_json::json!({
        "request": { "name": schema.request_name.to_string(), "fields": jsonify(&schema.request_fields) },
        "response": { "name": schema.response_name.to_string(), "fields": jsonify(&schema.response_fields) }
    }).to_string()
}
