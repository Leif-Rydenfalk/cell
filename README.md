# Cell – Biological Distributed Super-computing Substrate

Run millions of zero-copy messages per second between sandboxed micro-services that behave like living cells.

---

## 30-second Demo

```bash
git clone https://github.com/Leif-Rydenfalk/cell
cd cell/examples/cell-market
cargo run --release
```

On a 2013 Intel i5 you should see ~9.5 M trades/sec processed by five auto-spawned trader cells talking to a consensus-backed exchange.

---

## What is Cell?

Cell is a **biologically-inspired** runtime for building **secure, high-throughput, distributed applications** in Rust.

* **Cell**     – a sandboxed process (Linux namespace / bwrap / Podman)  
* **Membrane** – the Unix-domain socket it listens on  
* **Vesicle**  – a zero-copy message (rkyv-serialised, mem-mapped)  
* **Synapse**  – a typed client that grows automatically  
* **DNA**      – the source code that is compiled on first use (cached in `~/.cell/cache`)  
* **Mycelium Root** – the host daemon that spawns cells on demand  

---

## Performance (single core, old hardware)

| Metric               | cell-market demo |
|----------------------|------------------|
| messages per second  | 9.5 M            |
| median latency       | <1 µs (Unix sock) |
| batch 100 messages   | 1 disk sync      |
| memory copy count    | 0 (rkyv archived) |

---

## Project Layout

```
cell/
├── cell-sdk/          # Runtime SDK (Membrane, Synapse, Vesicle, …)
├── cell-consensus/    # Embeddable Raft + batched WAL
├── cell-macros/       # `#[protein]` and `signal_receptor!` for codegen
└── examples/          # Living demos
    └── cell-market/   # 9 M TPS market simulation
```

---

## Writing a Cell

1. **Define the protocol**

```rust
use cell_sdk::protein;

#[protein]
pub enum PingMsg {
    Ping(u64),
    Pong(u64),
}
```

2. **Implement the cell**

```rust
use cell_sdk::{Membrane, vesicle::Vesicle};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Membrane::bind("pong", |v: Vesicle| async move {
        let ping = rkyv::from_bytes::<PingMsg>(v.as_slice())?;
        let pong = PingMsg::Pong(ping.0);
        Ok(Vesicle::wrap(rkyv::to_bytes::<_,16>(&pong)?.into_vec()))
    }).await
}
```

3. **Call it from anywhere**

```rust
let mut syn = Synapse::grow("pong").await?;
let reply = syn.fire(PingMsg::Ping(42)).await?;
```

---

## Security Model

* **No network by default** – cells share a Unix socket directory only  
* **Read-only root-fs** – code cannot be modified at runtime  
* **User-namespace mapping** – files created by the container belong to the host user  
* **Resource limits** – CPU / memory cgroup quotas (Podman path)  
* **Automatic sandbox escape prevention** – `bwrap --unshare-all --die-with-parent`

---

## How it Works

1. **Mycelium Root** listens on `~/.cell/run/mitosis.sock`  
2. `Synapse::grow("name")` asks Root to spawn the binary if the socket is absent  
3. Root compiles the DNA (incremental, cached) and launches the cell inside Capsid (bwrap)  
4. Cell binds its Membrane socket (`/tmp/cell/name.sock` inside the container, `~/.cell/run/name.sock` on host)  
5. Messages are rkyv-serialised, sent over the Unix socket, and **zero-copy deserialised** on the receiver side  
6. Optional: embed `cell-consensus` for disk-backed Raft consensus with **batch-append WAL** (single `fsync` per batch)

---

## Requirements

* Linux 5.10+ (for `memfd`, `user-ns`, `cgroup v2`)  
* Rust 1.75+  
* bubblewrap (`bwrap`) installed (or Podman for rootless containers)  

---

## Environment Variables

| Variable           | Purpose                                      |
|--------------------|----------------------------------------------|
| `CELL_SOCKET_DIR`  | Override socket directory (default `~/.cell/run`) |
| `CELL_UMBILICAL`   | Override Mycelium Root socket                |
| `CELL_GOLGI_SOCK`  | Used by QUIC transport (future)              |

---

## Road-map / Ideas

* QUIC-based inter-host pheromone routing  
* GPU vesicles (CUDA memory-mapped buffers)  
* eBPF packet filtering inside Membrane  
* Hot swap DNA without dropping connections  
* Web-assembly ribosome (compile once, run anywhere)

---

## License

MIT OR Apache-2.0  
Built with ❤️ and a touch of chlorophyll.
