Here is the comprehensive plan to re-architect Cell into a universal, biological substrate that scales from 50-year-old mainframes to 10-cent microcontrollers, while keeping your current demos running exactly as they are.

# The "Run on Everything" Master Plan

**Objective:**
Split the monolithic runtime into biological layers.
*   **Nucleus (`cell-model`):** Pure logic. No OS dependencies. (`no_std`).
*   **Nervous System (`cell-transport`):** Configurable I/O. Gated features for Linux organs vs. Embedded organs.
*   **Body (`cell-sdk`):** The facade that re-assembles the parts based on the environment.

---

## 1. The Nucleus: `cell-model` (Pure `no_std`)

We must strip this crate of all OS concepts. It defines **what** a cell is, not **how** it talks.

*   **Remove:** `dirs`, `anyhow`, `thiserror`. These depend on the OS or `std`.
*   **Remove:** `resolve_socket_dir`. Paths are a filesystem concept.
*   **Add:** `no-std-compat` or generic `Error` types.
*   **Add:** `#[no_std]` attribute to `lib.rs`.
*   **Result:** This crate can now compile on a bare-metal ARM Cortex-M4. It provides `Vesicle`, `Protocol`, and the `rkyv` schemas.

## 2. The Nervous System: `cell-transport` (The Switchboard)

This is where the magic happens. Instead of hardcoding `quinn` or `unix sockets`, we define the `Synapse` as a wrapper around *available* organs.

### The Feature Gates (Cargo.toml)
We define the environment via features:
*   `std`: Enables OS primitives (Filesystem, Threads).
*   `shm`: Enables Shared Memory (Requires `std` + Linux).
*   `axon`: Enables QUIC Network (Requires `std` + `tokio`).
*   `alloc`: Enables Heap (Vec/Box).

### The Gated Enum (The API Glue)
To keep the API "exact same" (`Synapse::grow`), we keep the `Transport` enum but guard its variants.

```rust
pub enum Transport {
    #[cfg(feature = "std")]
    Socket(UnixStream),
    
    #[cfg(feature = "shm")]
    SharedMemory { ... },
    
    #[cfg(feature = "axon")]
    Quic(quinn::Connection),
    
    // Future: #[cfg(feature = "uart")]
    // Serial(UartDevice)
}
```

### The Abstraction (`NervousSystem` Trait)
We introduce an internal trait `NervousSystem` to unify behavior.
*   `UnixStream`, `ShmClient`, and `QuicConnection` will all implement `send` / `recv`.
*   This prepares the codebase for custom embedded transports without breaking the current API.

## 3. The Body: `cell-sdk` (The Assembler)

The SDK creates the default experience.

*   **Default Features:** `["std", "shm", "axon", "process"]`.
    *   This ensures your demos (Exchange/Trader) compile exactly as they do today.
*   **Embedded Build:** Users can opt-out: `default-features = false, features = ["alloc"]`.
    *   This gives them a lightweight client library compatible with embedded systems.

---

## Implementation Steps

### Phase 1: Purify the Core (`cell-model`)
1.  **Delete Dependencies:** Remove `dirs` and error crates from `cell-model/Cargo.toml`.
2.  **Refactor Errors:** Replace `thiserror` derives with manual `Display` impls or a lightweight `no_std` error crate.
3.  **Evict Logic:** Move `resolve_socket_dir` to `cell-transport`.
4.  **Verify:** Ensure `cell-model` compiles with `cargo build --no-default-features`.

### Phase 2: Modularize Transport (`cell-transport`)
1.  **Gate Imports:** Wrap all `tokio`, `nix`, `quinn` imports in `#[cfg(feature = "...")]` blocks.
2.  **Gate Modules:**
    *   `shm.rs` → `#[cfg(feature = "shm")]`
    *   `membrane.rs` → `#[cfg(feature = "std")]` (Requires socket binding)
3.  **Refactor Synapse:**
    *   Update `enum Transport` to use `cfg` attributes.
    *   Update `Synapse::grow` logic. If `std` is missing, `grow` should probably error or default to a manual connection method (since auto-discovery requires FS/Net).

### Phase 3: The Axon Diet (`cell-axon`)
1.  **JSON Purge:** As previously discussed, replace `serde_json` with `rkyv` in `pheromones.rs`.
2.  **Gate Everything:** Ensure the entire crate is harmless if the `axon` feature is disabled in the workspace.

### Phase 4: Lifecycle Isolation (`cell-process`)
1.  **Gate It:** Ensure this crate is only included when `std` is active. It is useless on embedded.

---

## The Result

**For your Demos (Linux Host):**
*   You run `cargo build`.
*   Cargo enables `default` features (`std`, `shm`, `axon`).
*   `Synapse` includes `UnixStream` and `Quinn`.
*   **Outcome:** 1.5M message/sec Zero-Copy performance. Same code.

**For the 50-Year Future (Microcontroller):**
*   You run `cargo build --no-default-features --features alloc`.
*   `cell-model` provides the binary protocol.
*   `Synapse` compiles without `tokio` or `nix`.
*   You implement a tiny `UartTransport` and plug it in.
*   **Outcome:** A 20KB binary running purely on metal, speaking the exact same language as the supercomputer.

This plan respects your requirement for **zero code changes in the demos** while successfully decoupling the architecture from the implementation details of the Linux kernel.