Design the SDK as a **fractal of tiny, replaceable crates** that all speak the **same 20-byte header + rkyv blob** on the wire.  
The user only touches the topmost façade (`cell-sdk`); everything underneath is opt-in via cargo features and trait crates.

```
cell-sdk                       ← façade crate, one-line dependency
│
├── cell-core                  ← #![no_std] + alloc. Only 4 items:
│   ├── Header                 │  struct Header { fingerprint: u64, len: u32, crc: u32 }
│   ├── Wire<T>                │  enum Wire<T> { Owned(&[u8]), ZeroCopy(&T::Archived) }
│   ├── Codec                  │  trait Codec { fn encode(&self) -> &[u8]; }
│   └── Transport              │  trait Transport { async fn call(&mut self, &[u8]) -> &[u8]; }
│
├── cell-codecs                ← implementations of Codec
│   ├── rkyv                   │  zero-copy
│   ├── postcard               │  ultra-small
│   └── cobs                   │  framing for UART
│
├── cell-transports            ← implementations of Transport
│   ├── unix-socket            │  std
│   ├── quic                   │  axon feature
│   ├── shm-ring               │  linux feature
│   ├── embassy-udp            │  nano feature
│   ├── embassy-uart           │  nano feature
│   └── can-bus                │  automotive feature
│
├── cell-discovery             ← plug-in trait
│   ├── local-table            │  static `&'static [SocketAddr]`
│   ├── udp-broadcast          │  LAN
│   ├── ble-beacon             │  nano
│   └── mavlink                │  drones
│
└── cell-sandbox               ← optional, swappable
    ├── bwrap                  │  Linux
    ├── rtt                    │  ARM Cortex-M (no MMU)
    └── wasm3                  │  Wasm3 interpreter
```

Rules

1. **Every crate depends only on `cell-core`.**  
   No transitive tokio, no std, no alloc unless explicitly enabled.

2. **Features are additive and orthogonal.**  
   `cargo add cell-sdk --features "nano,uart,cobs"` gives you a 20 kB binary that talks COBS-framed postcard over UART.

3. **Traits are sealed inside `cell-core` except `Transport` and `Codec`.**  
   Adding a new radio is:  
   ```
   impl Transport for MyLora { ... }
   ```

4. **The façade re-exports a **type alias** that picks the right impls at compile time:**

   ```rust
   #[cfg(feature = "nano")]
   pub type Synapse = cell_transports::embassy_udp::UdpSynapse<cell_codecs::postcard::Postcard>;

   #[cfg(all(feature = "std", not(feature = "nano")))]
   pub type Synapse = cell_transports::unix::UnixSynapse<cell_codecs::rkyv::Rkyv>;
   ```

5. **Wire format never changes.**  
   Always `Header(20 B) + payload`. New transports only wrap it; new codecs only change the bytes inside.

6. **Build scripts auto-generate the glue.**  
   A `build.rs` in `cell-sdk` emits a tiny `impl_dispatch.rs` that wires the selected `Transport + Codec` into the public `Synapse::fire` method, so the user still writes:

   ```rust
   let mut s = Synapse::grow("imu").await?;
   let resp: ImuData = s.fire(&ImuRequest).await?;
   ```
   * handled by cell macros

Result  
You can land Cell on a Mars rover by adding one line to `Cargo.toml`, and you can land it on a 1 cm spy drone by changing **one** line.  
Everything else—discovery, sandbox, transport—is a Lego brick that snaps onto the immutable 20-byte header.