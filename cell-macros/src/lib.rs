extern crate proc_macro;
use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use std::fs;
use std::path::PathBuf;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parse_macro_input, DeriveInput, Expr, Field, Ident, Token, Type};

/// The #[protein] attribute acts as the biological building block for data.
///
/// It automatically derives:
/// - Serde (Serialize, Deserialize) for Polyglot/JSON support.
/// - Rkyv (Archive, Serialize, Deserialize) for Zero-Copy Rust support.
/// - Debug and Clone.
///
/// It also enforces `check_bytes` for security to prevent undefined behavior
/// when reading untrusted memory.
#[proc_macro_attribute]
pub fn protein(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // We use DeriveInput to support both Structs and Enums
    let input = parse_macro_input!(item as DeriveInput);

    let expanded = quote! {
        #[derive(
            // JSON / Polyglot Support
            ::serde::Serialize,
            ::serde::Deserialize,
            // Zero-Copy Rust Support
            ::cell_sdk::rkyv::Archive,
            ::cell_sdk::rkyv::Serialize,
            ::cell_sdk::rkyv::Deserialize,
            // Standard Traits
            Clone,
            Debug,
        )]
        // Point to the specific rkyv version re-exported by cell_sdk
        #[archive(crate = "::cell_sdk::rkyv")]
        // Security: Validation of incoming bytes is mandatory
        #[archive(check_bytes)]
        // Ensure the Zero-Copy view is also Debuggable
        #[archive_attr(derive(Debug))]
        #input
    };

    TokenStream::from(expanded)
}

#[proc_macro]
pub fn signal_receptor(input: TokenStream) -> TokenStream {
    let schema = parse_macro_input!(input as ReceptorDef);
    let req_name = &schema.input_name;
    let req_fields = &schema.input_fields;
    let resp_name = &schema.output_name;
    let resp_fields = &schema.output_fields;

    let schema_json = generate_json(&schema);

    // Note: We manually apply the attributes here instead of using #[protein]
    // because we are generating the structs from scratch, not decorating existing ones.
    let expanded = quote! {
        #[derive(
            ::serde::Serialize,
            ::serde::Deserialize,
            ::cell_sdk::rkyv::Archive,
            ::cell_sdk::rkyv::Serialize,
            ::cell_sdk::rkyv::Deserialize,
            Clone,
            Debug
        )]
        #[archive(crate = "::cell_sdk::rkyv")]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        pub struct #req_name { #(pub #req_fields),* }

        #[derive(
            ::serde::Serialize,
            ::serde::Deserialize,
            ::cell_sdk::rkyv::Archive,
            ::cell_sdk::rkyv::Serialize,
            ::cell_sdk::rkyv::Deserialize,
            Clone,
            Debug
        )]
        #[archive(crate = "::cell_sdk::rkyv")]
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

    match parse_and_generate_call(&schema_json, &cell_str, &signal_data) {
        Ok(ts) => TokenStream::from(ts),
        Err(e) => TokenStream::from(quote! { compile_error!(#e) }),
    }
}

// --- Internal Parsing Logic ---

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
    let json_val: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let output_type_str = json_val["output"]
        .as_str()
        .ok_or("Missing 'output' in genome")?;
    let output_ident = syn::Ident::new(output_type_str, proc_macro2::Span::call_site());

    Ok(quote! {
        (move || -> ::anyhow::Result<#output_ident> {
            let payload = ::cell_sdk::rkyv::to_bytes::<_, 1024>(&#expr)
                .map_err(|e| ::anyhow::anyhow!("Packing error: {}", e))?
                .into_vec();
            let v_out = ::cell_sdk::vesicle::Vesicle::wrap(payload);

            let mut synapse = ::cell_sdk::Synapse::grow(#cell_name)?;
            let v_in = synapse.fire(v_out)?;

            let archived = ::cell_sdk::rkyv::check_archived_root::<#output_ident>(v_in.as_slice())
                .map_err(|e| ::anyhow::anyhow!("Validation error: {}", e))?;

            use ::cell_sdk::rkyv::Deserialize;
            let resp: #output_ident = archived.deserialize(&mut ::cell_sdk::rkyv::Infallible)
                .map_err(|e| ::anyhow::anyhow!("Deserialization error: {}", e))?;

            Ok(resp)
        })()
    })
}
