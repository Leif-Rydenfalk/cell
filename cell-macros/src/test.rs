// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

pub fn cell_test_impl(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let fn_name = &input.sig.ident;
    let block = &input.block;

    // This macro transforms a test function into a standalone executable entrypoint
    // that the Cell Test Runner expects.
    // It creates a main function that initializes the Cell Runtime properly.
    
    let expanded = quote! {
        // We compile this as a bin, so we need a main.
        #[tokio::main]
        async fn main() -> ::anyhow::Result<()> {
            ::cell_sdk::tracing::info!("Test Cell Booting...");
            
            // Hydrate Identity immediately (Standard Cell Behavior)
            let _config = ::cell_sdk::identity::Identity::get();
            
            // Run the test logic
            let ctx = ::cell_sdk::CellTestContext::new(stringify!(#fn_name));
            
            match test_logic(ctx).await {
                Ok(_) => {
                    ::cell_sdk::tracing::info!("TEST PASSED");
                    Ok(())
                }
                Err(e) => {
                    ::cell_sdk::tracing::error!("TEST FAILED: {}", e);
                    Err(e)
                }
            }
        }

        async fn test_logic(ctx: ::cell_sdk::CellTestContext) -> ::anyhow::Result<()> {
            #block
            Ok(())
        }
    };

    TokenStream::from(expanded)
}