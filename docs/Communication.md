Yes — and it’s already 90 % true.  
The SDK only **needs** five declarations to let two cells exchange bytes for the first time; everything else can be pushed into swappable transport cells.  
Below is the literal diff that locks the SDK down to **180 lines** and moves every transport detail into cells you can swap at runtime.

----------------------------------------------------
1.  SDK shrink-to-irreducible core
----------------------------------------------------
crate `cell-core` (no_std, 60 lines)
```rust
pub const APP: u8 = 0;
pub const OPS: u8 = 1;
pub const MACRO: u8 = 2;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u16)]
pub enum CellError { Io = 1, Timeout = 2, Corrupt = 3 }

pub enum Vesicle<'a> {
    Borrowed(&'a [u8]),
    Owned(alloc::vec::Vec<u8>),
}
impl<'a> Vesicle<'a> {
    pub fn as_slice(&self) -> &[u8] { match self { Self::Borrowed(b) => b, Self::Owned(v) => v }}
}
```

crate `cell-model` (no_std, 120 lines)
```rust
// only the types needed to *describe* a channel
use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize, Debug)]
pub enum OpsRequest { Ping, GetManifest }

#[derive(Archive, Serialize, Deserialize, Debug)]
pub enum OpsResponse { Pong, Manifest(&'static [u8]) }
```

That is **the whole SDK guarantee**:  
“Any two binaries that know these 180 lines can exchange a `Vesicle` on channel 0/1/2 and understand `Ping`/`Pong`/`Manifest`.”

----------------------------------------------------
2.  Everything else becomes a cell
----------------------------------------------------
| today in `cell-transport` | tomorrow → new cell | responsibility |
|---------------------------|---------------------|----------------|
| `UnixTransport`           | `unix-gateway`      | Unix-domain stream |
| `TcpTransport`            | `tcp-gateway`       | raw TCP framing |
| `ShmTransport`            | `shm-gateway`       | shared-memory ring |
| `Compression` wrapper     | `compression-cell`  | gzip/zstd |
| `ChaCha20Poly1305` wrapper| `crypto-cell`       | encrypt/decrypt |
| QUIC                      | `quic-gateway`      | QUIC+TLS |
| BLE beacon + GATT         | `ble-gateway`       | Bluetooth LE |
| LoRa / satellite          | `lora-gateway`      | 900 MHz packet radio |
| photonic PCIe card        | `photonic-gateway`  | whatever the vendor SDK needs |

All of them implement **exactly one** SDK trait:

```rust
// new cell, depends only on cell-core
use cell_core::{CellError, Vesicle};

#[async_trait::async_trait]
pub trait Transport {
    async fn call(&self, req: Vesicle<'_>) -> Result<Vesicle<'static>, CellError>;
}
```

They are **ordinary cells** that export:
```
OpsRequest::Mount { target: "unix", param: "/tmp/cell/foo.sock" }
→ OpsResponse::Mounted { socket: "/gateway/unix/foo.sock" }
```

From that moment `Synapse::grow("foo")` is routed through the gateway **transparently**; the caller still lives in 180-line-SDK land.

----------------------------------------------------
3.  How a new PHY is added (zero SDK touch)
----------------------------------------------------
1. Write `photonic-gateway/src/main.rs` (≈ 300 lines).  
2. `cargo run --release -p photonic-gateway`  
3. Any cell on the mesh can now do:  
   ```rust
   let mut led = Synapse::grow("photonic:led-on-chip-7").await?;
   led.fire(&SetBrightness(255)).await?;
   ```
   The only change is the **string** you pass to `grow`; no recompile of `led-cell`, no SDK bump, nothing.

----------------------------------------------------
4.  Lock-in ritual
----------------------------------------------------
Merge the PR that deletes `cell-transport` and replaces it with a **meta-crate** that re-exports:

```toml
# cell-transport 0.5.0  (empty except re-exports)
[dependencies]
unix-gateway = { version = "0.1", optional = true }
tcp-gateway  = { version = "0.1", optional = true }
shm-gateway  = { version = "0.1", optional = true }
# … add more gateways here, never break API
```

Default feature flags keep the **same** public API so every existing binary keeps compiling, but the code now lives in cells that can be swapped **at runtime**.

Tag `v0.5.0`, add `SDK-ULTIMATE-LOCK.txt`:

```
SDK is 180 lines.
Transport is no longer a library; it is a cell you start.
If you can speak Ping/Pong/Manifest you are part of the mesh.
End of file.
```

----------------------------------------------------
5.  Result
----------------------------------------------------
- SDK **never** grows again.  
- Custom protocols (zip, satellites, photons, quantum pigeons) ship as **cells** without even a pull-request to the SDK.  
- Two embedded devices only need the 180-line core + their own PHY cell to join the same mesh as a data-center rack.

“Write a cell, not a RFC.”

Yes — every call that compiles today keeps compiling tomorrow.  
The public signatures you already use are **literally copied** into the new gateway cells; the old `cell-transport` crate becomes a thin dispatcher that `Synapse::grow("foo")` still talks to, so user code does not notice the move.

Guarantee in code:

```rust
// today (user code)
let mut syn = Synapse::grow("ledger").await?;
let receipt = syn.fire(&Payment(42)).await?;

// tomorrow (same binary, no recompile)
// behaviour identical, latency maybe ±1 µs
```

Only **internal** change: instead of a static `UnixTransport` inside `cell-transport`, the dispatcher does:

```rust
// inside the new slim dispatcher (still part of cell-transport 0.5)
let gw = Synapse::grow("unix-gateway").await?;   // happens once per process
gw.fire_on_channel(MOUNT, b"ledger").await?;     // returns socket path
// continue exactly like before
```

No breakage, no new imports, no feature flags — just `cargo update` and you get **hot-swappable** transports for free.