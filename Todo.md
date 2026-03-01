 if e.to_string().contains("Broken pipe") {
                    // Force reconnection on next tick by clearing the client
                    let mut renderer_guard = self.renderer.lock().await;
                    *renderer_guard = None; // Will reconnect in tick()
                }


We should get the typed error code / enum here - it should be typed and we should know instantly that the pipe is broken and handle it accordingly.




 Here's how to add Ping/Pong by default in the SDK:

## 1. Add Ping/Pong to `cell-sdk/src/lib.rs`

```rust
// cell-sdk/src/lib.rs
// Add these at the top level

/// Standard health check request
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(check_bytes)]
#[archive(crate = "rkyv")]
pub struct Ping;

/// Standard health check response
#[derive(Debug, Clone, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[archive(check_bytes)]
#[archive(crate = "rkyv")]
pub struct Pong {
    pub timestamp: u64,
}

impl Pong {
    pub fn now() -> Self {
        Self {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

// Re-export in prelude
pub mod prelude {
    pub use super::serde::{Deserialize, Serialize};
    pub use super::{
        anyhow::{Error, Result},
        cell_remote,
        config::CellConfig,
        expand,
        handler,
        protein,
        resilient_synapse::{ResilienceConfig, ResilientSynapse},
        runtime::Runtime,
        service,
        Membrane,
        Ping,      // <-- ADD
        Pong,      // <-- ADD
        ResilientSynapse as Synapse,
    };
}
```

## 2. Add Ping handler to the Membrane by default

Update `cell-sdk/src/membrane.rs` to automatically handle Ping requests:

```rust
// cell-sdk/src/membrane.rs

use crate::{BoxFuture, Ping, Pong};
// ... other imports

impl Membrane {
    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
        _opts: Option<()>,
        _conf: Option<()>,
        _coord: Option<()>,
    ) -> Result<()>
    where
        F: for<'a> Fn(&'a Req::Archived) -> BoxFuture<'a, Result<Resp>>
            + Send
            + Sync
            + 'static
            + Clone,
        Req: Archive + Send + 'static,
        for<'a> Req::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>
            + Send
            + Sync
            + 'static,
        Resp: rkyv::Serialize<AllocSerializer<1024>> + Send + 'static,
    {
        // Wrap the user's handler with automatic Ping handling
        let wrapped_handler = move |channel: u8, payload: &[u8]| {
            let handler = handler.clone();
            Box::pin(async move {
                if channel == channel::APP {
                    // Try to parse as Ping first
                    if let Ok(archived) = rkyv::check_archived_root::<Ping>(payload) {
                        // It's a ping! Respond with pong
                        let _ = archived; // Unused but ensures validation
                        let response = Pong::now();
                        let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)
                            .map_err(|_| anyhow::anyhow!("Failed to serialize Pong"))?
                            .into_vec();
                        return Ok(resp_bytes);
                    }
                    
                    // Not a ping, forward to user handler
                    match rkyv::check_archived_root::<Req>(payload) {
                        Ok(archived) => {
                            let response = handler(archived).await?;
                            let resp_bytes = rkyv::to_bytes::<_, 1024>(&response)
                                .map_err(|_| anyhow::anyhow!("Failed to serialize response"))?
                                .into_vec();
                            Ok(resp_bytes)
                        }
                        Err(e) => {
                            Err(anyhow::anyhow!("Request validation failed: {:?}", e))
                        }
                    }
                } else {
                    // Non-APP channel - pass through raw
                    Err(anyhow::anyhow!("Unsupported channel: {}", channel))
                }
            })
        };
        
        // ... rest of the bind implementation using wrapped_handler
    }
}
```

## 3. Add automatic Ping to the cell_remote! macro

Update your `cell_remote!` macro to automatically include a ping method:

```rust
#[proc_macro]
pub fn cell_remote(input: TokenStream) -> TokenStream {
    // ... parsing code ...
    
    let output = quote! {
        use std::collections::HashMap;
        
        #[derive(Debug, Clone)]
        pub struct #client_name {
            inner: std::sync::Arc<tokio::sync::Mutex<Option<crate::CellClient>>>,
            service_name: &'static str,
        }
        
        impl #client_name {
            pub async fn connect() -> Result<Self, crate::anyhow::Error> {
                Ok(Self {
                    inner: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
                    service_name: #service_name,
                })
            }
            
            /// Ping the remote cell to check if it's alive
            /// Returns the timestamp from the remote cell
            pub async fn ping(&self) -> Result<crate::Pong, crate::anyhow::Error> {
                self.call("ping", crate::Ping).await
            }
            
            // ... rest of the implementation
        }
    };
    
    output.into()
}
```

## 4. Add default Ping handler to the service macro

Update your `#[service]` macro to automatically implement a ping method:

```rust
// In cell-macros/src/service.rs
#[proc_macro_attribute]
pub fn service(_args: TokenStream, input: TokenStream) -> TokenStream {
    // Parse the struct
    
    let output = quote! {
        #input
        
        impl #struct_name {
            // Auto-generated ping handler
            pub async fn ping(&self, _req: crate::Ping) -> Result<crate::Pong, crate::anyhow::Error> {
                Ok(crate::Pong::now())
            }
        }
    };
    
    output.into()
}
```

## 5. Update the handler macro to route ping requests

```rust
// In cell-macros/src/handler.rs
// Modify the generated dispatch code to route "ping" to the ping method
```

## 6. Add Ping to the CellClient

Update `cell-sdk/src/client.rs` (or wherever CellClient is defined):

```rust
impl CellClient {
    // ... existing methods ...
    
    /// Convenience method to ping a remote cell
    pub async fn ping(&self, cell_name: &str) -> Result<Pong, anyhow::Error> {
        let synapse = ResilientSynapse::grow(cell_name).await?;
        synapse.fire(&Ping).await?.deserialize()
    }
}
```

## 7. Simplified Usage in Cells

With all these changes, cells can now:

1. **Implement ping automatically** - no code needed
2. **Call ping automatically** - via the generated client
3. **No need to define Ping/Pong in every cell**

Your cell code becomes much cleaner:

```rust
// No need to define Ping/Pong structs anymore!
// No need to implement ping handler!

#[handler]
impl MyService {
    // Your actual business logic methods
    async fn do_something(&self, req: SomeRequest) -> Result<SomeResponse> {
        // ...
    }
}
```

And the generated client works automatically:

```rust
// The cell_remote! macro now includes ping automatically
cell_remote!(MyRemote = "my-remote");

// Usage
let client = MyRemote::connect().await?;
let pong = client.ping().await?;  // Works automatically!
println!("Remote timestamp: {}", pong.timestamp);
```

This provides a clean, zero-boilerplate health check system across all cells.