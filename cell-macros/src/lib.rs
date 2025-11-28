extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use std::fs;
use std::path::PathBuf;
use syn::parse::{Parse, ParseStream, Parser};
use syn::{braced, parse_macro_input, DeriveInput, Expr, Field, Ident, LitStr, Token, Type};

// Helper to parse attributes: #[protein(class = "Exchange", version = 1)]
struct ProteinArgs {
    class: Option<String>,
}

impl ProteinArgs {
    fn parse(attr: TokenStream) -> Self {
        let mut class = None;
        if !attr.is_empty() {
            // simplistic parser for example purposes
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

    // 1. Calculate Deterministic Hash of the AST
    // We strip whitespace/comments by using the Debug repr of the AST or a specific traversal.
    // For this proof-of-concept, we hash the stringified token stream which effectively
    // fingerprints the structure definition.
    let ast_string = quote!(#input).to_string();
    let mut hasher = blake3::Hasher::new();
    hasher.update(ast_string.as_bytes());
    let hash_bytes = hasher.finalize();
    let hash_hex = hash_bytes.to_hex().to_string();
    let fp_bytes: [u8; 8] = hash_bytes.as_bytes()[0..8].try_into().unwrap(); // Take first 8 bytes
    let fp_u64 = u64::from_le_bytes(fp_bytes);

    // 2. Side-Effect: Publish or Verify Schema
    // This happens during compilation on the host machine.
    if let Some(class_name) = args.class {
        let home = dirs::home_dir().expect("No home dir");
        let schema_dir = home.join(".cell/schema");
        let _ = fs::create_dir_all(&schema_dir);
        let lock_file = schema_dir.join(format!("{}.lock", class_name));

        // Logic: The first one to build (the authority) writes the lock.
        // Subsequent builds (clients) verify against it.
        // In a real system, you might separate #[protein(authority="...")] vs #[protein(client="...")]
        // Here we use a simpler heuristic: If it exists, verify. If not, create.

        if lock_file.exists() {
            let expected_hash = fs::read_to_string(&lock_file).expect("Failed to read lockfile");
            if expected_hash.trim() != hash_hex {
                // *** THE MAGIC: COMPILE TIME FAILURE ***
                return syn::Error::new_spanned(
                    struct_name,
                    format!(
                        "SCHEMA MISMATCH! \n\
                        Remote '{}' expects hash: {} \n\
                        Local struct produces:    {} \n\
                        Did the server update? Run 'cell clean' or update your struct definition.",
                        class_name,
                        expected_hash.trim(),
                        hash_hex
                    ),
                )
                .to_compile_error()
                .into();
            }
        } else {
            // We are likely the server/authority, or the first one building.
            fs::write(&lock_file, &hash_hex).expect("Failed to write schema lock");
        }
    }

    // 3. Generate Code
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

        // Embed the fingerprint in the binary for runtime checks too
        impl #struct_name {
            pub const SCHEMA_FINGERPRINT: u64 = #fp_u64;
        }
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

    let expanded = quote! {
        #[derive(
            ::cell_sdk::serde::Serialize,
            ::cell_sdk::serde::Deserialize,
            ::cell_sdk::rkyv::Archive,
            ::cell_sdk::rkyv::Serialize,
            ::cell_sdk::rkyv::Deserialize,
            Clone,
            Debug
        )]
        #[serde(crate = "::cell_sdk::serde")]
        #[archive(crate = "::cell_sdk::rkyv")]
        #[archive(check_bytes)]
        #[archive_attr(derive(Debug))]
        pub struct #req_name { #(pub #req_fields),* }

        #[derive(
            ::cell_sdk::serde::Serialize,
            ::cell_sdk::serde::Deserialize,
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
        .join(".cell")
        .join("data")
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
    #[allow(dead_code)] // name is parsed but not currently used in generation
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
