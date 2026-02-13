 if e.to_string().contains("Broken pipe") {
                    // Force reconnection on next tick by clearing the client
                    let mut renderer_guard = self.renderer.lock().await;
                    *renderer_guard = None; // Will reconnect in tick()
                }


We should get the typed error code / enum here - it should be typed and we should know instantly that the pipe is broken and handle it accordingly.




 
 
 
 Looking at the error logs and the code, I can see the issue. The **World cell** is getting "Broken pipe" errors when trying to update celestial body positions via RPC to the **Renderer cell**. This happens because:

1. The **Renderer cell** crashed or was restarted
2. The **World cell** still holds a stale `Renderer::Client` connection
3. The SDK's `Synapse`/`Client` doesn't automatically reconnect or try alternative transports

Let me analyze the SDK code to understand why reconnection isn't happening:

## Root Cause Analysis

Looking at `/home/asdfghj/cell-1/cell-sdk/src/synapse.rs`:

```rust
pub async fn grow(cell_name: &str) -> Result<Self> {
    // ... connection setup ...
    let mut transport = Transport::Socket(Arc::new(Mutex::new(stream)));

    if let Ok(shm_client) = Self::try_upgrade_to_shm(&mut transport).await {
        tracing::info!("Synapse upgraded to SHM for neighbor: {}", cell_name);
        transport = Transport::Shm(shm_client);
    }

    Ok(Self { my_id, transport })
}
```

The `Synapse` is created **once** and never retries. If the connection breaks, it's dead.

Looking at `/home/asdfghj/cell-1/cell-sdk/src/io_client.rs`:

```rust
pub async fn connect(target: &str) -> Result<std::os::unix::net::UnixStream> {
    // Retry configuration
    const MAX_RETRIES: u32 = 10;
    const INITIAL_DELAY_MS: u64 = 100;
    // ...
}
```

The `IoClient::connect` **does** have retry logic, but it's only used during **initial** connection establishment, not for **reconnection** after failure.

## The Core Problem

The SDK architecture has these issues:

1. **`Synapse` is immutable after creation** - No reconnection logic
2. **`cell_remote!` generated clients hold a `Synapse` permanently** - When it breaks, the client is dead
3. **No health checking** - The client doesn't detect the connection is dead until a request fails
4. **No fallback to alternative transports** - If socket fails, it doesn't try to re-establish via IO cell or neighbor links

## Why the SDK Doesn't Fix It

Looking at the generated client code in `/home/asdfghj/cell-1/cell-macros/src/lib.rs`:

```rust
pub async fn #name(&self, #(#arg_sigs),*) -> ::anyhow::Result<#ret_type> {
    let req = #protocol_name::#variant_name { #(#arg_names),* };
    
    // FIX: Deref the Arc to get the Synapse
    let resp_wrapper = (*self.conn).fire(&req).await
        .map_err(|e| ::anyhow::anyhow!("RPC Error: {}", e))?;
    // ...
}
```

The client just returns the error - no retry, no reconnection. The `Synapse::fire` method in `synapse.rs` just fails if the transport is broken.

## What Should Happen (But Doesn't)

The SDK **should**:

1. **Detect transport failures** and mark connections as unhealthy
2. **Automatically reconnect** using the same logic as initial connection (neighbor links → IO cell → global registry)
3. **Retry with backoff** for transient failures
4. **Potentially downgrade/upgrade transports** - if SHM fails, fall back to socket; if socket fails, try to re-establish

Looking at `/home/asdfghj/cell-1/cell-sdk/src/connection_manager.rs`, there **is** sophisticated connection management code with:
- Circuit breakers
- Health checks
- Retry logic with exponential backoff
- Connection pooling

But it's **not integrated** into the basic `Synapse` or `cell_remote!` generated clients!

## The Fix Needed

The `Synapse` needs to be wrapped in a `ConnectionManager` or have reconnection logic built-in. The `cell_remote!` macro should generate clients that:

```rust
// Instead of:
let resp_wrapper = (*self.conn).fire(&req).await?; // Fails permanently

// Should be something like:
let resp_wrapper = self.conn.fire_with_retry(&req).await?; // Retries, reconnects
```

Or the `Synapse` itself should be `ResilientSynapse` that handles reconnection transparently.

## Quick Fix for Your World Cell

As a workaround, you can modify your World cell to detect failures and recreate the client:

```rust
// In world/src/main.rs, modify update_positions:

async fn update_positions(&self, orbital: &Orbital::Client, renderer: &Renderer::Client) -> Result<()> {
    let bodies = orbital.get_bodies(Orbital::GetBodies).await?;
    
    let mut updated = 0;
    for body in bodies.bodies {
        // ... transform calculation ...
        
        match renderer.update_transform(...).await {
            Ok(_) => updated += 1,
            Err(e) => {
                tracing::error!("[World] Failed to update {}: {}", body.name, e);
                // DETECT BROKEN PIPE - need to reconnect!
                if e.to_string().contains("Broken pipe") {
                    // Force reconnection on next tick by clearing the client
                    let mut renderer_guard = self.renderer.lock().await;
                    *renderer_guard = None; // Will reconnect in tick()
                }
            }
        }
    }
    Ok(())
}
```

But this is hacky. The **real fix** is in the SDK's `Synapse` or `cell_remote!` macro to add automatic reconnection with the existing retry infrastructure in `connection_manager.rs`.