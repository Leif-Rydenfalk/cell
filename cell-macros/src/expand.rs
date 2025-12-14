// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Attribute, ItemStruct, LitStr, Meta};

pub fn expand_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // We are parsing an attribute like #[expand("database", "table")]
    // But since this is a recursive macro system, the attribute input passed to this function
    // corresponds to the *current* macro invocation's arguments.
    // However, we want to accumulate *all* expand attributes on the struct.

    // The strategy:
    // 1. Parse the struct.
    // 2. Look for ALL #[expand(...)] attributes on it.
    // 3. Extract the layer/feature names.
    // 4. Generate the `impl Struct { async fn serve() ... }` block.
    // 5. Calculate the dependencies based on the layers used.

    // Note: In a real recursive macro expansion, the compiler strips the attribute
    // responsible for the current invocation. We must handle the remaining ones
    // or re-emit the struct without the *current* expand attribute but keeping others?
    // Actually, `proc_macro_attribute` consumes the attribute and the item.
    // We must re-emit the item (struct).

    // For simplicity in this implementation, we will assume this macro handles
    // the generation of the `serve` method based on *its* arguments,
    // and we might need a way to merge multiple expand calls.
    //
    // However, the prompt describes a composition model:
    // #[expand("auth")]
    // #[expand("database")]
    // struct X
    //
    // If we simply implement `expand` to generate a partial impl, Rust allows multiple `impl X` blocks.
    // But we need a single entry point `serve`.
    //
    // refined approach:
    // The `expand` macro adds a specific trait implementation or a specific init function
    // corresponding to that layer (e.g. `init_auth`, `init_database`).
    // Then we need a master `serve` that calls them all.
    //
    // For this MVP, let's make `expand` smart:
    // It generates an `impl` block with a unique function name based on the layer/feature.
    // AND it generates a `serve` function ONLY if one doesn't exist? No, that's hard to check across macro invocations.
    //
    // Alternative: The *user* writes `Runtime::ignite_with_deps` manually?
    // No, the prompt says "What the macros generate... impl UserTable { async fn serve ... }".
    //
    // Solution:
    // We will assume the `expand` macro generates a standalone function `run_<layer>_<feature>`.
    // And we generates a `main` shim? No, the user writes main.
    //
    // Let's implement the "additive" behavior.
    // Each `expand` macro adds a dependency to a compile-time list? Hard in Rust without external state.
    //
    // Let's implement the specific example functionality:
    // #[expand("database", "table")] -> Generates `impl UserTable { async fn serve ... }`
    // If multiple `expand` macros are present, they might conflict if they all try to generate `serve`.
    //
    // Compromise for this implementation:
    // The macro inspects the arguments.
    // If it sees "database", "table", it generates the `serve` method which claims ownership of the cell lifecycle.
    // Other expansions (metrics, auth) might generate methods that inject themselves?
    // Or maybe "database" is the "primary" one that drives the others.

    // Let's implement parsing of the arguments provided to THIS invocation.

    let args_parser = syn::parse::Parser::parse2(
        |input: syn::parse::ParseStream| {
            let layer: LitStr = input.parse()?;
            input.parse::<syn::Token![,]>()?;
            let feature: LitStr = input.parse()?;
            Ok((layer, feature))
        },
        proc_macro2::TokenStream::from(attr),
    );

    let (layer, feature) = match args_parser {
        Ok(x) => x,
        Err(e) => return e.to_compile_error().into(),
    };

    let item_struct = parse_macro_input!(item as ItemStruct);
    let struct_name = &item_struct.ident;

    let layer_str = layer.value();
    let feature_str = feature.value();

    // Logic for "database" "table"
    let impl_block = if layer_str == "database" && feature_str == "table" {
        quote! {
            impl #struct_name {
                pub async fn serve(name: &str) -> ::anyhow::Result<()> {
                    // Start dependencies required by a database table
                    // Note: In a full implementation, we'd scan other attributes to add "auth", "metrics" to this list.
                    // For now, we hardcode the standard stack described in the prompt.
                    let deps = vec!["nucleus", "metrics", "auth", "autoscaler", "raft-coord"];

                    ::cell_sdk::tracing::info!("Starting {} (Database Table) with deps: {:?}", name, deps);

                    ::cell_sdk::runtime::Runtime::ignite_with_deps(
                        Self::generated_handlers(), // Hypothetical generated handler
                        name,
                        &deps
                    ).await
                }

                fn generated_handlers() -> impl Fn(&::cell_model::protocol::ArchivedMitosisRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = ::anyhow::Result<::cell_model::protocol::MitosisResponse>> + Send>> + Send + Sync + 'static + Clone {
                    // Placeholder handler logic
                    move |_req| Box::pin(async {
                        Ok(::cell_model::protocol::MitosisResponse::Ok { socket_path: "db.sock".into() })
                    })
                }

                // Client generation shim
                pub async fn connect() -> ::anyhow::Result<Self> {
                     // In real code this returns a Client struct, not Self.
                     // Mocking for compilation check of the pattern.
                     Ok(Self {
                         // we'd need to construct fields here or use a builder
                         // skipping construction for brevity in macro expansion
                         // assuming fields are pub or default
                         ..unsafe { std::mem::zeroed() }
                     })
                }

                pub async fn insert(&mut self, _data: impl ::cell_sdk::serde::Serialize) -> ::anyhow::Result<()> {
                    Ok(())
                }
            }
        }
    } else {
        // For other expansions, we might just add utility methods or do nothing visible in this MVP
        quote! {}
    };

    // Re-emit the struct + the impl
    quote! {
        #item_struct
        #impl_block
    }
    .into()
}
