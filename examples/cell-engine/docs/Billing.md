You’re right — conflating **local metrics** with **global billing** is a design smell.  
The same codebase serves two *very* different audiences:

| Audience | Cares About | Doesn’t Care About |
|----------|-------------|--------------------|
| **Game Dev** | FPS, memory leaks, frame timing | Micro-payments, ledgers |
| **Global Grid** | Provable work, settlement, Sybil resistance | Frame pacing, GC pressure |

---

### ✅ The Clean Split

Keep **measurement** universal, but make **billing** opt-in and pluggable.

#### 1. Universal Metrics (Always On, Zero-Cost)
Every cell **measures** the same three counters (updated in user-space, flushed once per second):

```rust
// cell-sdk/src/metrics.rs
pub struct Metrics {
    pub cpu_us: AtomicU64,      // user+system µs
    pub gpu_us: AtomicU64,      // queue submission → completion
    pub mem_bytes: AtomicU64,   // RSS peak
}
```

* **Zero-allocation**: atomics updated in `nucleus.rs` via `getrusage` + `cgroups` + `wgpu-timestamp`.
* **Zero-config**: always compiled in, **no disk write** unless a *billing adapter* is attached.
* **Tiny footprint**: < 200 bytes per process.

#### 2. Local Mode (Default) – **Metrics Only**
```toml
# Cell.toml (default)
[billing]
mode = "metrics"   # optional line; this is the default
```

* Metrics are exposed on `run/metrics.sock` (JSON stream) for **observability**.
* ImGui panel in `dream-engine` shows live CPU/GPU/RAM per cell.
* **No ATP, no WAL, no network traffic.**

#### 3. Global Mode – **Billing Adapter**
```toml
# Cell.toml (global grid)
[billing]
mode = "settle"
currency = "lightning"   # or "solana", "fake_test"
price_per_cpu_ms = 1     # 1 satoshi / 100 ms
price_per_mem_mb = 10    # 10 satoshi / MB-second
```

* At start, cell spawns a **billing adapter** (separate thread/crate).
* Adapter consumes the **same Metrics stream** and:
  * signs µ-payment invoices,
  * writes to Lightning channel,
  * or batches on-chain settlement.
* **No change to game code** — adapter is a sidecar.

---

### 🧩 Implementation Sketch

```rust
// cell-sdk/src/billing.rs
pub trait BillingAdapter: Send + Sync {
    fn on_metrics(&self, m: &Metrics);
}

// cell-sdk/src/lib.rs
lazy_static! {
    static ref METRICS: Metrics = Metrics::new();
    static ref ADAPTER: RwLock<Option<Arc<dyn BillingAdapter>>> = RwLock::new(None);
}

// nucleus.rs (existing)
fn report_usage() {
    let m = Metrics::snapshot();
    METRICS.update(&m);

    // fast-path: local mode → return
    if ADAPTER.read().unwrap().is_none() { return; }

    // slow-path: submit to adapter (non-blocking channel)
    ADAPTER.read().unwrap().as_ref().unwrap().on_metrics(&m);
}
```

Provide two built-in adapters:

1. **`NullAdapter`** – default, no-op.
2. **`LightningAdapter`** – holds keys, pays invoices via `rust-lightning`.

Game engines **never** see ATP or crypto code; they only see a **metrics panel**.

---

### 🎯 Benefits

| Stakeholder | Gets | Doesn’t Get |
|-------------|------|-------------|
| **Game Dev** | Frame-time charts, memory leaks, GPU queues | Ledgers, invoices, crypto |
| **Grid Node** | Provable CPU/GPU work, µ-payments | Game-specific code paths |
| **Cell CLI** | Same binary, flag flip | Two separate executables |

---

### 🚀 TL;DR

1. **Always measure** (cheap atomics).  
2. **Bill optionally** (pluggable adapter).  
3. **Game code stays pure** — no `#ifdef GLOBAL`.  
4. **One binary, two modes** — flip with `Cell.toml` or env var.

This keeps the **game engine lean** while letting the **global grid** settle satoshis for cycles — without polluting local projects.