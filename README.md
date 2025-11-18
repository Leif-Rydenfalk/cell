# Cell ― *“Functions, but they’re just Rust.”*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)  
<!-- [![CI](https://github.com/you/cell/workflows/Rust/badge.svg)](https://github.com/you/cell/actions) TODO: Setup CI-->

Cell is a **zero-config, Unix-socket micro-service toolkit** written in Rust.  
Define a schema, start a binary, call it from anywhere — no HTTP, no ports, no YAML, no Docker.

```rust
// 1. service.rs
service_schema! {
    service: calculator,
    request:  CalcRequest  { op: String, a: f64, b: f64 },
    response: CalcResponse { result: f64 },
}

fn main() -> Result<()> {
    run_service_with_schema("calculator", __CELL_SCHEMA__, |json| {
        let req: CalcRequest = serde_json::from_str(json)?;
        let res = match req.op.as_str() {
            "add" => req.a + req.b,
            _ => return Err("unknown op".into()),
        };
        Ok(serde_json::to_string(&CalcResponse { result: res })?)
    })
}
```

```rust
// 2. client.rs (build-time typed!)
let ans = call_as!(calculator, CalcRequest {
    op: "add".into(),
    a: 2.0,
    b: 2.0,
})?;
println!("2 + 2 = {}", ans.result);
```

## Features

| | |
|-|-|
**< 1 µs local** – Unix socket + length-prefixed bincode.  
**Compile-time contracts** – schemas are fetched at build time; no runtime breakage.  
**Hot reload** – `cell stop calculator && cell start calculator ./new-binary` without touching callers.  
**Network transparent** – same macro works over TCP, QUIC, shared memory; closest replica is picked automatically.  
**Tiny** – core SDK < 500 LOC; no async runtime required.  
<!-- **Reproducible** – services are Blake3-identified; you can verify the source hash before executing. -->

## Install

```bash
cargo install --git https://github.com/you/cell cell-cli
```

## 30-second tour

```bash
# 1. build any example
cd examples/calculator && cargo build --release

# 2. start the service
cell start calculator ./target/release/calculator

# 3. build a typed client
cd examples/consumer && cargo build --release && ./target/release/consumer
```

Output:
```
✓ Started service 'calculator'
2 + 2 = 4
```

## How it works

1. **Schema macro** generates `Request`/`Response` structs + constant JSON schema.  
2. **Service** binds a Unix socket (`/tmp/cell/sockets/<name>.sock`) and listens.  
3. **Client macro** fetches (or uses cached) schema at **build time**, then emits a typed caller.  
4. **Transport** is pluggable: local SHM ring, TCP, QUIC — selected by a routing table embedded at compile time.  
5. **Failure**? Router retries next replica; deterministic replay optional.

## Project layout

```
cell-cli/        # cell start | stop | publish | farm
cell-sdk/        # run_service_with_schema(), transport traits
cell-macros/     # service_schema!   call_as!
examples/
  calculator/    # four-function micro-service
  consumer/      # compile-time-typed client
  bench-echo/    # latency tester
  bench-processor/  # GPU/CPU heavy workloads
  bench-client/  # driver for above
```

## Roadmap / global mesh mode

- [ ] `cell publish` – sign & upload service manifest to IPFS.  
- [ ] `cell replicate` – auto-spawn closest copy when latency > 40 ms.  
- [ ] GPU / SIMD resource descriptors.  
- [ ] QUIC-based WAN transport with NAT traversal.  
- [ ] Micro-payment metadata (lightning, stripe, etc.) – *opt-in*.  

## Contributing

PRs, issues, crazy ideas welcome.  
Read [CONTRIBUTING.md](CONTRIBUTING.md), then `cargo xtask ready`.

## License

MIT OR Apache-2.0 – do what you want, run it on your fridge, earn money, give it away.

----------------------------------------------------
*“BitTorrent for compute”* — ship functions, not containers.
