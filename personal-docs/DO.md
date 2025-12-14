 The codebase is a **miniature distributed operating system**: consensus, RPC, discovery, encryption, billing, polyglot support, and a macro-driven SDK. That’s not a toy—it’s a **launchable substrate**.

But it’s also **raw plasma**. The next 48 hours should be about **hardening, pruning, and proving** it works under load. Below is a **battle-plan** ranked by “will kill you in production” → “nice to have”.

--------------------------------------------------------------------
1. Kill-the-System Test (next 2 h)
--------------------------------------------------------------------
Goal: find the first thing that crashes at 100 % CPU or 1 k conn/s.

```bash
# 1. Build release binaries
cargo build --release -p cell-cli -p cell-consensus

# 2. Single-node soak
RUST_LOG=info ./target/release/cell mitosis examples/cell-mesh --donor &
echo $! > pid.txt
timeout 60s ./target/release/cell-bench-client --qps 5000 --size 4KB
kill $(cat pid.txt)

# 3. Check for
#    - panic backtraces
#    - memory climb (RSS)
#    - dead-locked threads (gdb -p <pid>, thread apply all bt)
```

If anything panics or leaks → fix that first (probably in `vacuole` or the TCP accept loop).

--------------------------------------------------------------------
2. WAL & Consensus Safety (next 4 h)
--------------------------------------------------------------------
- **Crash test**: `kill -9` the leader mid-write, restart, assert no fork (checksum the final kv store on two followers).  
- **Corruption test**: flip one bit in the last WAL record → node must refuse to start and print a clear error.  
- **Unit-test**: truncate WAL at every possible boundary (length, CRC, payload) → must return empty, not panic.

--------------------------------------------------------------------
3. Network & Crypto Hardening (next 3 h)
--------------------------------------------------------------------
- **Noise XX replay**: send the same handshake twice → must reject second connection.  
- **QUIC migration**: replace raw TCP in `synapse.rs` with Quinn; you already depend on `rustls`/`quinn` in `transport.rs`, just wire it up.  
- **Pheromone flood**: 1 k fake multicast packets/sec → must not grow unbounded memory in the `HashMap` of peers (add TTL + LRU eviction).

--------------------------------------------------------------------
4. Resource Governance (next 3 h)
--------------------------------------------------------------------
- **CPU throttle**: use `cgroups-rs` to cap a cell at 0.5 core → verify `cpu_time_ms` in `Metabolism` plateaus.  
- **Memory cap**: same for 100 MB RSS → OOM-kill must be reflected in `max_rss_kb`.  
- **ATP bankruptcy**: if balance < 0, refuse incoming `burn_atp` calls with `Error::InsufficientATP`.

--------------------------------------------------------------------
5. Observability (next 2 h)
--------------------------------------------------------------------
Add three **cheap** metrics endpoints:

```rust
// In golgi/mod.rs
lazy_static::latency_histogram: HDRHistogram = ...
// on every request
latency_histogram.record(duration.as_micros() as u64);
```

Expose via Unix socket `run/metrics.sock`:

```json
{"rps": 3120, "p50_us": 420, "p99_us": 810, "errors": 0, "balance_atp": 1337}
```

--------------------------------------------------------------------
6. Packaging & CI (next 2 h)
--------------------------------------------------------------------
- **Dockerfile** (multi-stage, 20 MB scratch image).  
- **GitHub Action** that runs the soak test on every push; fail if latency > 5 ms p50 or any panic.  
- **Release script**: `cargo deb` + `cargo rpm` so you can `apt install cell`.

--------------------------------------------------------------------
7. Documentation & Demo (next 4 h)
--------------------------------------------------------------------
- **One-page README**: animated asciinema showing `membrane mitosis` booting 3 nodes, `call_as!` RPC in 5 lines.  
- **Benchmark numbers** you just captured (e.g., 1.2 M req/s on 8 cores, 4 KB msg, <1 ms p99 local).  
- **Roadmap issue** on GitHub listing “v0.3” features (below).

--------------------------------------------------------------------
8. v0.3 Roadmap (after you ship 0.2)
--------------------------------------------------------------------
1. **Raft leader election** – swap the current “all-write” for real consensus.  
2. **Horizontal autoscale** – Golgi watches CPU > 70 % and spawns new replicas via `nucleus::activate`.  
3. **WebAssembly nucleus** – run `*.wasm` instead of native binaries (sandbox + portability).  
4. **Payment channel** – Lightning-style µATP invoices so strangers can pay for cycles.  
5. **GUI dashboard** – real-time mesh graph, ATP flow, per-cell flame-graphs.

--------------------------------------------------------------------
TL;DR
--------------------------------------------------------------------
You are **one hardening sprint** away from a 0.2 release that others can `git clone && cargo run --release`.  
Pick the top three bullets in sections 1-3, hammer them for 24 h, tag `v0.2.0`, then tweet the benchmark numbers.  
After that, **ship the roadmap** and you’ll have a living distributed OS, not a weekend hack.