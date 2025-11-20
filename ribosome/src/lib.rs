extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use std::fs;
use std::path::PathBuf;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parse_macro_input, Expr, Field, Ident, Token, Type};

#[proc_macro]
pub fn signal_receptor(input: TokenStream) -> TokenStream {
    let schema = parse_macro_input!(input as ReceptorDef);
    let req_name = &schema.input_name;
    let req_fields = &schema.input_fields;
    let resp_name = &schema.output_name;
    let resp_fields = &schema.output_fields;

    let schema_json = generate_json(&schema);

    let expanded = quote! {
        #[derive(::serde::Serialize, ::serde::Deserialize, ::cytosol::rkyv::Archive, ::cytosol::rkyv::Serialize, ::cytosol::rkyv::Deserialize, Debug, Clone)]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        pub struct #req_name { #(pub #req_fields),* }

        #[derive(::serde::Serialize, ::serde::Deserialize, ::cytosol::rkyv::Archive, ::cytosol::rkyv::Serialize, ::cytosol::rkyv::Deserialize, Debug, Clone)]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        pub struct #resp_name { #(pub #resp_fields),* }

        #[doc(hidden)]
        pub const __GENOME__: &str = #schema_json;
    };
    TokenStream::from(expanded)
}

#[proc_macro]
pub fn call_as(input: TokenStream) -> TokenStream {
    let CallArgs {
        cell_name,
        signal_data,
    } = parse_macro_input!(input as CallArgs);
    let cell_str = cell_name.to_string();

    // 1. Locate Genome Snapshot
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let schema_path = PathBuf::from(&manifest_dir)
        .join(".cell-genomes")
        .join(format!("{}.json", cell_str));

    let schema_json = match fs::read_to_string(&schema_path) {
        Ok(content) => content,
        Err(_) => {
            return TokenStream::from(
                quote! { compile_error!("Missing genome snapshot. Run 'membrane mitosis' to sync.") },
            )
        }
    };

    // 2. Generate Typed Call Block
    match parse_and_generate_call(&schema_json, &cell_str, &signal_data) {
        Ok(ts) => TokenStream::from(ts),
        Err(e) => TokenStream::from(quote! { compile_error!(#e) }),
    }
}

// --- Internal Parsers ---

struct ReceptorDef {
    name: Ident,
    input_name: Ident,
    input_fields: Vec<Field>,
    output_name: Ident,
    output_fields: Vec<Field>,
}

impl Parse for ReceptorDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Format: name: foo, input: Bar { ... }, output: Baz { ... }
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

fn generate_json(def: &ReceptorDef) -> String {
    // Simplified JSON for Schema Sync
    // In production, we'd use serde_json and a real struct
    format!(
        r#"{{ "input": "{}", "output": "{}" }}"#,
        def.input_name, def.output_name
    )
}

fn parse_and_generate_call(
    json: &str,
    cell_name: &str,
    expr: &Expr,
) -> Result<proc_macro2::TokenStream, String> {
    // Quick parse of JSON to find Output Type Name
    // In a real impl, use serde_json::Value
    let json_val: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let output_type_str = json_val["output"]
        .as_str()
        .ok_or("Missing 'output' in genome")?;
    let output_ident = syn::Ident::new(output_type_str, proc_macro2::Span::call_site());

    Ok(quote! {
        {
            // 1. Pack Vesicle
            let payload = ::cytosol::rkyv::to_bytes::<_, 1024>(&#expr)
                .map_err(|e| ::anyhow::anyhow!("Packing error: {}", e))?
                .into_vec();
            let v_out = ::cytosol::vesicle::Vesicle::wrap(payload);

            // 2. Grow Synapse & Fire
            let mut synapse = ::cytosol::Synapse::grow(#cell_name)?;
            let v_in = synapse.fire(v_out)?;

            // 3. Unpack Response
            let archived = ::cytosol::rkyv::check_archived_root::<#output_ident>(v_in.as_slice())
                .map_err(|e| ::anyhow::anyhow!("Validation error: {}", e))?;

            use ::cytosol::rkyv::Deserialize;
            let resp: #output_ident = archived.deserialize(&mut ::cytosol::rkyv::Infallible)
                .map_err(|e| ::anyhow::anyhow!("Deserialization error: {}", e))?;

            Ok::<#output_ident, ::anyhow::Error>(resp)
        }
    })
}
