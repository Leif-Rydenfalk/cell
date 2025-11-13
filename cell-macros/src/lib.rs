extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Type, Token, Ident};
use syn::parse::{Parse, ParseStream};

// ... (keep all the service_schema and parsing code exactly the same) ...

#[proc_macro]
pub fn service_schema(input: TokenStream) -> TokenStream {
    let schema = parse_macro_input!(input as ServiceSchema);
    
    let request_name = &schema.request_name;
    let request_fields = &schema.request_fields;
    
    let response_name = &schema.response_name;
    let response_fields = &schema.response_fields;
    
    let schema_json = generate_schema_json(&schema);
    let schema_hash = blake3::hash(schema_json.as_bytes()).to_hex().to_string();
    
    let expanded = quote! {
        #[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
        pub struct #request_name {
            #(pub #request_fields),*
        }
        
        #[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
        pub struct #response_name {
            #(pub #response_fields),*
        }
        
        #[doc(hidden)]
        pub const __CELL_SCHEMA__: &str = #schema_json;
        
        #[doc(hidden)]
        pub const __CELL_SCHEMA_HASH__: &str = #schema_hash;
    };
    
    TokenStream::from(expanded)
}

struct ServiceSchema {
    request_name: Ident,
    request_fields: Vec<syn::Field>,
    response_name: Ident,
    response_fields: Vec<syn::Field>,
}

impl Parse for ServiceSchema {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let _service_name = input.parse::<Ident>()?;
        input.parse::<Token![,]>()?;
        
        input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let request_name = input.parse::<Ident>()?;
        
        let request_content;
        syn::braced!(request_content in input);
        let mut request_fields = Vec::new();
        
        while !request_content.is_empty() {
            let field_name = request_content.parse::<Ident>()?;
            request_content.parse::<Token![:]>()?;
            let field_type = request_content.parse::<Type>()?;
            
            request_fields.push(syn::Field {
                attrs: vec![],
                vis: syn::Visibility::Inherited,
                mutability: syn::FieldMutability::None,
                ident: Some(field_name),
                colon_token: Some(Default::default()),
                ty: field_type,
            });
            
            if request_content.peek(Token![,]) {
                request_content.parse::<Token![,]>()?;
            }
        }
        
        input.parse::<Token![,]>()?;
        
        input.parse::<Ident>()?;
        input.parse::<Token![:]>()?;
        let response_name = input.parse::<Ident>()?;
        
        let response_content;
        syn::braced!(response_content in input);
        let mut response_fields = Vec::new();
        
        while !response_content.is_empty() {
            let field_name = response_content.parse::<Ident>()?;
            response_content.parse::<Token![:]>()?;
            let field_type = response_content.parse::<Type>()?;
            
            response_fields.push(syn::Field {
                attrs: vec![],
                vis: syn::Visibility::Inherited,
                mutability: syn::FieldMutability::None,
                ident: Some(field_name),
                colon_token: Some(Default::default()),
                ty: field_type,
            });
            
            if response_content.peek(Token![,]) {
                response_content.parse::<Token![,]>()?;
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

fn type_to_string(ty: &Type) -> String {
    quote::quote!(#ty).to_string()
}

fn generate_schema_json(schema: &ServiceSchema) -> String {
    let mut req_fields = Vec::new();
    for field in &schema.request_fields {
        let name = field.ident.as_ref().unwrap().to_string();
        let ty = type_to_string(&field.ty);
        req_fields.push(serde_json::json!({
            "name": name,
            "type": ty
        }));
    }
    
    let mut resp_fields = Vec::new();
    for field in &schema.response_fields {
        let name = field.ident.as_ref().unwrap().to_string();
        let ty = type_to_string(&field.ty);
        resp_fields.push(serde_json::json!({
            "name": name,
            "type": ty
        }));
    }
    
    serde_json::json!({
        "request": {
            "name": schema.request_name.to_string(),
            "fields": req_fields
        },
        "response": {
            "name": schema.response_name.to_string(),
            "fields": resp_fields
        }
    }).to_string()
}

#[proc_macro]
pub fn call_as(input: TokenStream) -> TokenStream {
    let call_args = parse_macro_input!(input as CallArgs);
    
    let service_name = call_args.service.to_string();
    
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let schema_path = std::path::Path::new(&out_dir).join(format!("{}_schema.json", service_name));
    
    if !schema_path.exists() {
        let error_msg = format!(
            "\n\n❌ No cached schema for service '{}'\n\
             \n\
             build.rs should have fetched it. Did build.rs run?\n\
             \n",
            service_name
        );
        
        return syn::Error::new_spanned(&call_args.service, error_msg)
            .to_compile_error()
            .into();
    }
    
    let schema_json = std::fs::read_to_string(&schema_path).expect("Failed to read schema");
    
    let (req_struct, req_name, resp_struct, resp_name) = generate_structs_from_schema(&schema_json);
    
    let service = &call_args.service;
    let request_expr = &call_args.request;
    
    // Generate call with length-prefixed framing
    let expanded = quote::quote! {
        {
            #req_struct
            #resp_struct
            
            (|| -> anyhow::Result<#resp_name> {
                use std::io::{Read, Write};
                
                let socket_path = format!("/tmp/cell/sockets/{}.sock", stringify!(#service));
                let mut stream = std::os::unix::net::UnixStream::connect(&socket_path)
                    .map_err(|e| anyhow::anyhow!("Failed to connect: {}", e))?;
                
                let request: #req_name = #request_expr;
                let json = serde_json::to_vec(&request)
                    .map_err(|e| anyhow::anyhow!("Serialize error: {}", e))?;
                
                // Write length-prefixed message
                stream.write_all(&(json.len() as u32).to_be_bytes())
                    .map_err(|e| anyhow::anyhow!("Write length error: {}", e))?;
                stream.write_all(&json)
                    .map_err(|e| anyhow::anyhow!("Write error: {}", e))?;
                stream.flush()
                    .map_err(|e| anyhow::anyhow!("Flush error: {}", e))?;
                
                // Read length-prefixed response
                let mut len_buf = [0u8; 4];
                stream.read_exact(&mut len_buf)
                    .map_err(|e| anyhow::anyhow!("Read length error: {}", e))?;
                
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut response_buf = vec![0u8; len];
                stream.read_exact(&mut response_buf)
                    .map_err(|e| anyhow::anyhow!("Read response error: {}", e))?;
                
                serde_json::from_slice(&response_buf)
                    .map_err(|e| anyhow::anyhow!("Deserialize error: {}", e))
            })()
        }
    };
    
    TokenStream::from(expanded)
}

struct CallArgs {
    service: Ident,
    request: syn::Expr,
}

impl Parse for CallArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let service = input.parse()?;
        input.parse::<Token![,]>()?;
        let request = input.parse()?;
        Ok(CallArgs { service, request })
    }
}

fn generate_structs_from_schema(
    schema_json: &str
) -> (proc_macro2::TokenStream, proc_macro2::Ident, proc_macro2::TokenStream, proc_macro2::Ident) {
    let schema: serde_json::Value = serde_json::from_str(schema_json)
        .expect("Invalid schema JSON");
    
    let req_name = schema["request"]["name"].as_str().unwrap();
    let req_name_ident = syn::Ident::new(req_name, proc_macro2::Span::call_site());
    
    let mut req_fields = Vec::new();
    for field in schema["request"]["fields"].as_array().unwrap() {
        let name = field["name"].as_str().unwrap();
        let ty_str = field["type"].as_str().unwrap();
        
        let field_ident = syn::Ident::new(name, proc_macro2::Span::call_site());
        let type_tokens: proc_macro2::TokenStream = ty_str.parse()
            .unwrap_or_else(|_| panic!("Failed to parse type: {}", ty_str));
        
        req_fields.push(quote::quote! { pub #field_ident: #type_tokens });
    }
    
    let request_struct = quote::quote! {
        #[derive(serde::Serialize, serde::Deserialize, Debug)]
        struct #req_name_ident {
            #(#req_fields),*
        }
    };
    
    let resp_name = schema["response"]["name"].as_str().unwrap();
    let resp_name_ident = syn::Ident::new(resp_name, proc_macro2::Span::call_site());
    
    let mut resp_fields = Vec::new();
    for field in schema["response"]["fields"].as_array().unwrap() {
        let name = field["name"].as_str().unwrap();
        let ty_str = field["type"].as_str().unwrap();
        
        let field_ident = syn::Ident::new(name, proc_macro2::Span::call_site());
        let type_tokens: proc_macro2::TokenStream = ty_str.parse()
            .unwrap_or_else(|_| panic!("Failed to parse type: {}", ty_str));
        
        resp_fields.push(quote::quote! { pub #field_ident: #type_tokens });
    }
    
    let response_struct = quote::quote! {
        #[derive(serde::Serialize, serde::Deserialize, Debug)]
        struct #resp_name_ident {
            #(#resp_fields),*
        }
    };
    
    (request_struct, req_name_ident, response_struct, resp_name_ident)
}
