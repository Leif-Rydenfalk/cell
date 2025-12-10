Here is the architectural documentation for Cell. It defines the philosophy, the constraints, and the two distinct workflows for managing Cell DNA.

# Cell Architecture & Developer Experience

## 1. The Philosophy: Biological Computing
Cell is not a microservice framework. It is a biological substrate.

*   **Shared Nothing:** Cells do not link to shared libraries (`common` crates). A Cell is a standalone organism. It defines its own DNA (Types/Protocols).
*   **Symbiosis:** If Cell A wants to talk to Cell B, it does not import Cell B's code. It **absorbs** Cell B's DNA interface and compiles a local client.
*   **Zero-Copy:** Communication on the same host happens via Shared Memory (SHM) ring buffers. Data is never copied, only pointers are passed.
*   **Discovery:** There is no central registry (Etcd/Zookeeper). Cells discover each other via Pheromones (UDP Broadcast/QUIC).

## 2. The "Dad in San Francisco" Constraint
A fundamental rule of Cell is **Decoupled Evolution**.

If you are writing an Engine in Norway, and your dad is writing a Ledger in San Francisco:
1.  You cannot rely on relative file paths (`../ledger`).
2.  You cannot rely on shared Cargo workspaces.
3.  You cannot assume his code compiles on your machine.

You only need his **DNA** (Interface Definition).

## 3. Workflow A: The Symbiote (Rapid Prototyping)
*Best for: Hackathons, Monorepos, Simple Services.*

In this mode, the `cell_remote!` macro acts as a "poor man's compiler." It reads the source code of the target cell directly.

```rust
// main.rs
// Reads "../ledger/src/main.rs", extracts #[protein], generates client.
cell_remote!(Ledger = "ledger"); 
```

### Limitations
1.  **Namespace Hell:** The macro blindly copies code. If the remote cell uses complex imports or aliases (`type X = Y`), the generated code might break in your crate.
2.  **Single File:** It expects the remote DNA to be defined in `main.rs` (or a flat structure).
3.  **Fragility:** Syntax errors in the remote cell cause macro expansion errors in your cell.

---

## 4. Workflow B: The Industrial (Production)
*Best for: Complex Projects, Team Environments, CI/CD.*

For advanced projects, we move logic out of the Macro and into `build.rs`. This follows the **Prost/Bindgen** pattern. This separates **DNA Extraction** (parsing) from **Compilation**.

### How it works
1.  **Export:** The Provider (Ledger) exports its DNA to a sanitized schema file.
2.  **Generate:** The Consumer (Engine) uses `build.rs` to generate a clean Rust client file.
3.  **Compile:** The Consumer includes the generated file.

### Step 1: The Build Script
Create a `build.rs` in your consumer crate.

```rust
// build.rs
fn main() {
    // 1. Point to the remote Cell (Local path, Git URL, or Registry)
    // 2. Generate a clean client file to OUT_DIR
    cell_build::configure()
        .register("ledger", "../ledger") // or "git://github.com/dad/ledger"
        .generate()
        .expect("Failed to generate Cell clients");
}
```

### Step 2: The Application Code
Instead of the macro doing the parsing, it simply includes the pre-generated, type-checked file.

```rust
// main.rs
mod clients {
    // Includes target/debug/build/engine-.../out/ledger_client.rs
    include!(concat!(env!("OUT_DIR"), "/ledger_client.rs"));
}

use clients::ledger::{Ledger, Asset}; // Fully typed, IDE-friendly
```

### Advantages
1.  **Robust Resolution:** The build tool can resolve modules, flatten imports, and sanitize the DNA before generating the client.
2.  **Better Errors:** If the schema is invalid, `build.rs` panics with a clear log before `rustc` even starts.
3.  **IDE Support:** Rust Analyzer handles `include!` inside modules much better than heavy procedural macro expansion.
4.  **Version Locking:** You can check the generated client into source control to ensure stability even if the remote source changes (snapshotting).

---

## 5. Performance Invariants (The Golden Rules)

Regardless of which workflow you use, you must adhere to these rules to maintain **Zero-Copy Performance**.

### Rule 1: The Gateway Tax
If a type needs to be sent over HTTP (via Gateway), the generated Protocol Enum must derive `Serialize` / `Deserialize`.
*   **Consequence:** The Wire Protocol must use owned types (`Vec<u8>`, `String`).
*   **The Macro's Job:** The macro automatically converts your handler signature `&Archived<Vec<u8>>` -> `Vec<u8>` for the Wire Protocol definition to satisfy Serde.

### Rule 2: The Zero-Copy Signature
To trigger the Zero-Copy path (SHM), your handler **must** use the specific `&Archived<T>` syntax.

**Slow (Allocation):**
```rust
// Macro generates: Vec<u8> -> Allocates memory -> Deserializes
async fn ingest(&self, data: Vec<u8>) 
```

**Fast (Zero-Copy):**
```rust
// Macro generates: Vec<u8> (Wire) -> &Archived<Vec<u8>> (Handler)
// Points directly into Ring Buffer. No allocation.
async fn ingest(&self, data: &Archived<Vec<u8>>) 
```

### Rule 3: No Allocations in the Hot Path
The SDK guarantees:
1.  **Zero-Alloc Write:** We reuse `AlignedVec` serialization buffers.
2.  **Zero-Alloc Read:** We return `Response::Borrowed` pointing to the internal socket buffer.
3.  **Vectorized Writes:** We use `write_all` (or vectored equivalent) to minimize syscalls.

If you see `malloc` in your flamegraph during a benchmark, it should be in **your business logic**, not the transport layer.

---

This is my first supercomputer substrate, I'm 17 hehe, can you tell me if this is the right api design: 

#[protein]
pub enum Asset { USD, BTC }

#[service]
struct LedgerService { ... }

#[handler]
impl LedgerService {
    async fn deposit(&self, user: u64, asset: Asset, amount: u64) -> Result<u64> {
        ...
    }
}

---

cell_remote!(Ledger = "ledger");

let mut ledger = Ledger::connect().await?;
let balance = ledger.deposit(42, Ledger::Asset::USD, 100).await??;

personally I love it and it will be much more powerful in the future but its a good start? what do you think?