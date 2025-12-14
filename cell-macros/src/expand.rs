// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_build::MacroRunner;
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct, LitStr};

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

    // Convert struct back to string to pass to runner
    let struct_source = quote! { #item_struct }.to_string();
    let layer_str = layer.value();
    let feature_str = feature.value();

    // Call the external macro runner via cell-build
    let generated_code = match MacroRunner::run(&layer_str, &feature_str, &struct_source) {
        Ok(code) => code,
        Err(e) => {
            let msg = format!(
                "Failed to expand macro '{}/{}': {}",
                layer_str, feature_str, e
            );
            return syn::Error::new(layer.span(), msg).to_compile_error().into();
        }
    };

    let generated_tokens: proc_macro2::TokenStream = match generated_code.parse() {
        Ok(t) => t,
        Err(e) => {
            return syn::Error::new(layer.span(), format!("Generated invalid Rust code: {}", e))
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
