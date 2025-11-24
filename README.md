This is **Cell**, a biological-inspired distributed supercomputing substrate written in Rust.  
Think of it as a **peer-to-peer, secure, micro-service mesh** that lets you:

* Declare services (“cells”) with a single macro (`signal_receptor!`).  
* Compile each cell to a native binary that speaks QUIC + Noise.  
* Boot an entire application cluster with one CLI command (`cell mitosis`).  
* Pay / get paid in “ATP” tokens for the CPU time you consume / provide.  
* Discover peers automatically via UDP multicast “pheromones” or a lighthouse.  
* Route traffic through Unix sockets locally and QUIC remotely with zero-copy serialization (rkyv).  
* Observe everything through a built-in log rotator (“vacuole”) and cgroup-aware billing.

Key components
--------------

| Folder/file | Purpose |
|-------------|---------|
| `cell-cli/` | The daemon / orchestrator binary (`membrane`).  Handles compilation, spawning, discovery, routing, crypto, billing. |
| `cell-sdk/` | Library you link into every cell.  Gives the `signal_receptor!` macro, `Membrane::bind`, `Synapse::grow`, zero-copy `Vesicle` containers. |
| `cell-macros/` | Procedural macros that generate the request/response types and the `call_as!` client code from a single declaration. |
| `examples/cell-bench/` | A tiny load-test: a “worker” cell that burns CPU and a “coordinator” cell that floods it with jobs. |
| `examples/cell-mesh/` | 50-replica chat-like mesh that measures 1-way latency at >10 kHz. |

Biological names map to CS concepts
-----------------------------------

| Biology | Computing |
|---------|-----------|
| Genome | `genome.toml` metadata (name, listen addr, replicas, dependencies). |
| Cell | One Rust binary that implements a `signal_receptor!`. |
| Nucleus | The `ChildGuard` that spawns the cell and tracks CPU / RSS via cgroups. |
| Mitochondria | Ledger file (`mitochondria.json`) that mints or burns ATP for every millisecond of work. |
| Golgi | QUIC + Unix-socket router that lives in every cell; does service discovery, load-balancing, crypto handshake, and billing. |
| Synapse | Noise-encrypted QUIC stream between two Golgi instances. |
| Vesicle | Aligned, zero-copy buffer used for messages. |
| Vacuole | Background log writer that rotates 10 MB files and applies back-pressure. |
| Pheromones | UDP multicast announcements (`239.255.0.1:9099`) containing cell name, public key, QUIC port, and donor flag. |
| Axon | TCP QUIC listener exposed to the outside world. |
| Gap junction | Unix-domain socket used for local IPC. |

Quick start (single machine)
----------------------------

```bash
# 1. Clone
git clone https://github.com/Leif-Rydenfalk/cell
cd cell

# 2. Build
cargo build --release

# 3. Run the bench cluster
cd examples/cell-bench
cargo run --bin membrane -- mitosis .               # boots 4 workers + coordinator
# in another terminal
cargo run --bin coordinator
```

You’ll see the coordinator discovering the workers via pheromones, then printing >100 kReq/s with sub-millisecond RTT on a laptop.

Security
--------

* Every cell has an Ed25519 keypair (`~/.cell/identity` or `run/identity`).  
* Handshake uses Noise `XX_25519_ChaChaPoly_BLAKE2s` and is **mutually authenticated**; the remote static key becomes the peer identity used for billing.  
* Self-signed certificates are generated on the fly and rotated per run; the cert SAN contains the cell name so the lighthouse can verify it.

Billing / tokenomics
--------------------

* 1 ATP = 100 ms of CPU time (wall-clock used as proxy in MVP).  
* ATP is **minted** by the donor when it finishes a job for a client.  
* ATP is **burned** by the client when it starts a job on a donor.  
* Negative balance is allowed (credit), but you can’t download new cells from the network without ATP.  
* Ledger is a simple JSON file; in a real deployment this would be a content-addressed chain.

Limitations / roadmap
---------------------

* MVP trusts the client’s measurement of CPU time; real version would read `/sys/fs/cgroup/.../cpu.stat`.  
* No garbage-collection of dead peers yet; pheromone table grows forever.  
* Lighthouse is a single UDP rendezvous server; replace with DHT.  
* No NAT-hole-punching beyond simple “both sides connect to lighthouse”.  
* ATP is not a real blockchain token (yet).  

But as a **zero-dependency, single-binary, <5 kLOC proof-of-concept**, Cell already shows:

* 100 k+ RPC/s on localhost (Unix sockets).  
* 10 k+ RPC/s between laptops over Wi-Fi (QUIC).  
* End-to-end encryption, service discovery, load-balancing, logging, and billing in one `cargo run`.

## License
MIT OR Apache-2.0 – do what you want.

## Author
17 y/o, self-taught. 