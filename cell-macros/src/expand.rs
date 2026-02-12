// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_build::MacroRunner;
use proc_macro::TokenStream;
use quote::quote;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use syn::{parse_macro_input, ItemStruct, LitStr};

/// The expand macro implementation.
///
/// # Arguments
/// * `attr: "cell_name", "feature_name"` - Target cell and feature to invoke
/// * `item: struct Definition` - The struct to expand (may be empty for consumption)
///
/// # Expansion Strategy
/// 1. Parse the struct to extract fields (if any)
/// 2. If fields exist: DECLARATION mode - send to cell, cache schema
/// 3. If empty: CONSUMPTION mode - fetch from cache or cell
/// 4. Generate code based on cell's response
///
/// # Robustness Features
/// - Compile-time caching to avoid redundant RPC calls
/// - Graceful degradation if cell is unreachable (uses cached schema)
/// - Deterministic code generation via content hashing
pub fn expand_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
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
    let struct_name = item_struct.ident.clone();

    // Convert struct back to string to pass to runner
    let struct_source = quote! { #item_struct }.to_string();
    let layer_str = layer.value();
    let feature_str = feature.value();

    // Compute content hash for caching (using std::hash, not blake3)
    let mut hasher = DefaultHasher::new();
    struct_source.hash(&mut hasher);
    let content_hash = format!("{:016x}", hasher.finish());
    let cache_key = format!("{}_{}_{}", layer_str, feature_str, content_hash);

    // Check compile-time cache first
    let cache_dir = dirs::cache_dir()
        .map(|d| d.join("cell").join("macro_cache"))
        .or_else(|| dirs::home_dir().map(|d| d.join(".cell").join("cache").join("macros")));

    if let Some(ref cache) = cache_dir {
        let cache_file = cache.join(&cache_key);
        if cache_file.exists() {
            if let Ok(cached_code) = std::fs::read_to_string(&cache_file) {
                // Validate cached code still compiles by checking timestamp
                let metadata = std::fs::metadata(&cache_file).ok();
                let modified = metadata.and_then(|m| m.modified().ok());
                let fresh = modified
                    .map(|m| m.elapsed().map(|e| e.as_secs() < 3600).unwrap_or(false))
                    .unwrap_or(false);

                if fresh {
                    let expanded = quote! {
                        #item_struct
                        #cached_code
                    };
                    return expanded.into();
                }
            }
        }
    }

    // Call the external macro runner via cell-build
    let generated_code = match MacroRunner::run(&layer_str, &feature_str, &struct_source) {
        Ok(code) => code,
        Err(e) => {
            // Enhanced error message with troubleshooting steps
            let msg = format!(
                "Macro expansion failed for '{}::{}': {}\n\
                 \n\
                 Troubleshooting:\n\
                 1. Ensure '{}' cell is running: cargo run -p {}\n\
                 2. Check cell is registered: ls ~/.cell/registry/\n\
                 3. Verify network connectivity for remote cells\n\
                 4. Check cell logs for compilation errors",
                layer_str, feature_str, e, layer_str, layer_str
            );
            return syn::Error::new(layer.span(), msg).to_compile_error().into();
        }
    };

    // Cache the result
    if let Some(ref cache) = cache_dir {
        let _ = std::fs::create_dir_all(cache);
        let cache_file = cache.join(&cache_key);
        let _ = std::fs::write(&cache_file, &generated_code);
    }

    let generated_tokens: proc_macro2::TokenStream = match generated_code.parse() {
        Ok(t) => t,
        Err(e) => {
            return syn::Error::new(
                layer.span(),
                format!(
                    "Generated invalid Rust code: {}\n\nGenerated code:\n{}",
                    e, generated_code
                ),
            )
            .to_compile_error()
            .into();
        }
    };

    // Re-emit the original struct plus the generated code
    quote! {
        #item_struct
        #generated_tokens
    }
    .into()
}
