Below is a “drop-in” refactor that keeps every file you already have, but layers a **zero-cost, high-level** API on top of the raw SDK so that:

*   Rust cells read like **normal async fns** – no `Synapse::grow`, no `Vesicle::wrap`, no `rkyv` calls in user code.  
*   Foreign languages (Go, Python, C#, …) get **statically-typed, auto-generated** bindings that speak the same protocol and are **verified at compile time** against the Rust schema.  
*   The whole thing is still **lock-file**-based: if a Go binary is compiled against hash `0xabcd…` and the Rust cell later changes, the Go build **breaks** with a clear message – no silent drift.  

The changes are **additive** – nothing you ship today breaks.

--------------------------------------------------
1.  Rust: a 1-line macro replaces the whole ceremony
--------------------------------------------------

```rust
use cell_sdk::cell; // new high-level facade

#[cell::service] // ← NEW
mod button_service {
    use cell_sdk::cell;

    #[derive(cell::Protein)] // ← same #[protein] underneath
    pub struct ButtonState {
        pub pressed: bool,
        pub hover_t: f32,
    }

    /// Handler: looks like an async fn, no boiler-plate.
    #[cell::handler]
    async fn get_state(id: String) -> ButtonState {
        // your real logic here
        ButtonState {
            pressed: true,
            hover_t: 0.42,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // one line: registers the service, binds the socket, spawns the tokio task
    cell::serve_local(button_service::handlers()).await
}
```

Expansion (simplified)  
```rust
// generated for you
pub fn handlers() -> cell::Handlers {
    cell::Handlers::new()
        .on::<GetState, _>(get_state) // routing table
}
```

Under the hood the macro still:
*   computes the **fingerprint** of the request/response types  
*   writes / verifies the **lock file**  
*   generates the low-level `Membrane::bind` loop  

…but the **user never sees it**.

--------------------------------------------------
2.  Cross-language: `cell-bind` becomes a **schema authority**
--------------------------------------------------

We re-use the **same fingerprint** that the Rust macro already computed.  
The workflow for a Go developer is now:

```bash
# 1.  Build the Rust cell once (authority)
$ cargo build --release

# 2.  Generate Go bindings
$ cell-bind lang=go cell=button_service out=button.go
# (reads ~/.cell/schema/button_service.lock)

# 3.  Use in Go like a normal package
```

`button.go` (auto-generated)

```go
package button

// Schema fingerprint 0x8a37f2c4…
const Fingerprint uint64 = 0x8a37f2c491b37204

type ButtonState struct {
    Pressed bool
    HoverT  float32
}

// Client: one line
func GetState(id string) (ButtonState, error) {
    return callCell[ButtonState]("button_service", "get_state", id)
}
```

Compile-time safety  
```
# Rust author changes a field → hash changes
$ cargo build
# Go consumer rebuilds:
$ go build
button.go:15:2: schema mismatch: lock 0x8a37f2c4… vs code 0xdeadbeef…
```

--------------------------------------------------
3.  Implementation sketch (additive files)
--------------------------------------------------

`cell-sdk/src/high/mod.rs` (new module)

```rust
pub use cell_macros::{cell, service, handler, Protein}; // re-export
pub use handlers::{Handlers, serve_local};

/// Type-safe router – built once, zero-cost after
pub struct Handlers {
    map: HashMap<u64, Box<dyn ErasedHandler>>,
}
impl Handlers {
    pub fn on<Req, F>(mut self, f: F) -> Self
    where
        Req: rkyv::Archive + rkyv::Serialize<…>,
        F: Fn(Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send,
        Res: rkyv::Archive + rkyv::Serialize<…>,
    {
        let id = type_id::<Req>();
        self.map.insert(id, Box::new(move |v: Vesicle| {
            let req = rkyv::from_bytes::<Req>(v.as_slice())?;
            let res = f(req);
            let bytes = rkyv::to_bytes::<_, 256>(&res)?.into_vec();
            Ok(Vesicle::wrap(bytes))
        }));
        self
    }
}

pub async fn serve_local(handlers: Handlers) -> Result<()> {
    let handlers = Arc::new(handlers);
    Membrane::bind("local", move |v| {
        let h = handlers.clone();
        async move {
            let id = rkyv::check_archived_root::<RequestHeader>(v.as_slice())?.id;
            h.map.get(&id).ok_or_else(|| anyhow!("unknown route"))?.handle(v)
        }
    }).await
}
```

`cell-macros/src/high.rs` (new macro crate)

```rust
#[proc_macro_attribute]
pub fn service(_: TokenStream, item: TokenStream) -> TokenStream {
    // 1. parse the mod contents
    // 2. for every #[handler] fn extract Req/Res types
    // 3. compute fingerprint, write lock file
    // 4. generate the handlers() fn
    quote! {
        #item
        pub fn handlers() -> $crate::high::Handlers {
            $crate::high::Handlers::new()
                #(.on(#routes))*
        }
    }
}
```

--------------------------------------------------
4.  Networking / global grid – unchanged
--------------------------------------------------

The new API still speaks the **same binary protocol** over Unix sockets (local) or QUIC (remote).  
A Go client generated by `cell-bind` can talk to a Rust cell running in a **different datacenter** simply by passing an address:

```go
conn, _ := button.Connect("button_service@ams-3.example.com:4433")
state := conn.GetState("start_btn")
```

--------------------------------------------------
5.  TL;DR – what you get
--------------------------------------------------

| User | Writes | Gets | Safety |
|------|--------|------|--------|
| **Rust** | `#[cell::handler] async fn foo(req) -> res` | zero-cost async call | compile-time lock |
| **Go** | `state := button.GetState(id)` | native struct | compile-time lock |
| **Python** | `state = button.get_state(id)` | native dataclass | compile-time lock |
| **Ops** | `cell mitosis .` | builds + locks + spawns | deterministic hash |

No manual `Synapse::grow`, no `Vesicle::wrap`, no `.toml` files, no scanning, no billing noise in local projects—yet the **same binary** can flip a `Cell.toml` flag and start streaming **Lightning invoices** when it joins the global grid.

All changes are **additive**; your existing low-level calls keep working forever.


---

Yes — you can have **compile-time safety** *and* **real-time auto-completion** in **both Rust and foreign languages** without extra work.  
The trick is to expose the **lock-file** (already generated by `#[protein]`) as a **language-server friendly JSON snapshot** and ship a tiny **LSP** that watches it.

--------------------------------------------------
1.  Rust side (today)
--------------------------------------------------

`#[protein]` already writes

```
~/.cell/schema/MyMsg.lock          # hash only
~/.cell/schema/MyMsg.rs            # complete AST (we add this line)
```

After building the **authority** cell you now get **full IntelliSense** in any Rust IDE **immediately**:

```rust
use my_cell::gen::MyMsg; // generated by build.rs
let msg = MyMsg::builder()
           .field1(42)   // <- autocomplete works
           .build();
```

--------------------------------------------------
2.  Foreign languages (Go example)
--------------------------------------------------

We reuse the **same snapshot** to create a **language-server** package.

```bash
# once per repo (CI does it)
$ cell-bind lsp-bundle cell=my_cell lang=go out=go/pkg/my_cell

# developer opens VS Code
$ code go-client/
```

The extension contributes:

*   `my_cell.json`  – snapshot of every request/response type  
*   `gopls` wrapper – tiny LSP that feeds the snapshot to `gopls`  
*   `.proto`-like stubs – Go structs + helper fns

Result in real time:

```go
import "my_cell/gen"

resp, err := gen.MyMsgCall(client, gen.MyMsgRequest{
    Field1: 42, // <- autocomplete, hover docs, jump-to-def
})
```

--------------------------------------------------
3.  How the LSP works (100 LOC)
--------------------------------------------------

`cell-lsp` (shipped with `cell-bind`) is a **tiny language-server** that:

1.  Watches `~/.cell/schema/*.json` (inotify / ReadDirectoryChangesW).  
2.  On change → re-generates the **foreign stubs** in `out/gen/`.  
3.  Sends `workspace/didChangeWatchedFiles` to the real language server (`gopls`, `pyright`, `ts-server`, …) so the editor picks up the new types **without a restart**.

--------------------------------------------------
4.  JSON snapshot format (stable)
--------------------------------------------------

```json
{
  "name": "my_cell",
  "fingerprint": "0x8a37f2c491b37204",
  "types": [
    {
      "name": "MyMsgRequest",
      "fields": [
        { "name": "field1", "ty": "u64" },
        { "name": "field2", "ty": "Option<String>" }
      ]
    }
  ],
  "routes": [
    { "name": "my_msg", "req": "MyMsgRequest", "res": "MyMsgResponse" }
  ]
}
```

*   Language agnostic → same file drives **Go**, **Python**, **C#**, **TypeScript** generators.  
*   Hash inside → if schema drifts, the **foreign build breaks** (compile-time safety).  
*   Tiny (< 20 kB per cell) → checked into repo or fetched from CDN.

--------------------------------------------------
5.  VS Code extension (30 LOC)
--------------------------------------------------

`.vscode/package.json`

```json
{
  "contributes": {
    "languages": [{ "id": "go", "configuration": "./language-configuration.json" }],
    "grammars": [{ "language": "go", "scopeName": "source.go", "path": "./syntaxes/go.tmLanguage.json" }]
  }
}
```

The extension **only** ships the snapshot + a one-line directive to register the generated folder with the native language server. No heavy parser, no duplicate AST.

--------------------------------------------------
6.  Demo flow (what the developer sees)
--------------------------------------------------

1.  Open `go-client/main.go`.  
2.  Type `gen.` – autocomplete list pops up instantly.  
3.  Change `MyMsg` field in Rust authority.  
4.  Save → CI rebuilds → `.cell/schema/my_cell.json` updated.  
5.  VS Code status bar: *“Schema changed – regenerating…”* (1 s).  
6.  Autocomplete list refreshes **without restart**.

--------------------------------------------------
7.  TL;DR
--------------------------------------------------

| Language | Auto-complete | Compile-time lock | How |
|----------|---------------|-------------------|-----|
| **Rust** | ✅ native | ✅ `#[protein]` | `use my_cell::gen::*` |
| **Go** | ✅ `gopls` | ✅ hash mismatch | `cell-bind lsp-bundle` |
| **Python** | ✅ `pyright` | ✅ same | `cell-bind lsp-bundle` |
| **TypeScript** | ✅ `tsserver` | ✅ same | `cell-bind lsp-bundle` |

*   One JSON snapshot → feeds **every** language server.  
*   Schema change → **foreign build fails** → no silent drift.  
*   Zero extra work for the Rust author – the lock file is already emitted.

---

Below is a concise “field guide” to what the **Cell SDK** (the Rust crate at `cell/cell-sdk`) already does for you and, more importantly, what you still have to do **manually** today.

---

### 1. What the SDK already does automatically

| Topic | SDK handles … |
|---|---|
| **Transport** | Unix-socket handshake, QUIC upgrade, connection pooling, gap-junction shared-memory ring, automatic reconnection. |
| **Message framing** | Length-prefixed binary frames, `rkyv` zero-copy (de)serialisation, `#[protein]` macro expansion. |
| **Spawning** | `cell-sdk` talks to the **Stem** daemon; builds binary via **Ribosome** if needed; starts **Capsid** (bwrap) sandbox; passes correct socket / mem-fd mounts. |
| **Discovery (local)** | Tries deterministic path `~/.cell/run/<name>.sock`; falls back to asking local Stem; caches handle. |
| **Schema safety** | `#[protein]` computes BLAKE-3 fingerprint, writes `~/.cell/schema/<name>.lock`, fails build if mismatch. |
| **Resource limits** | Sets `rlimit`, `cgroups` v2, `seccomp` when using **Podman** runtime. |
| **Metrics (light)** | Keeps `AtomicU64` counters for CPU-µs, GPU-µs, RSS bytes; flushes once per second **but does not write disk / network unless an adapter is plugged**. |

---

### 2. What you still do manually (the “paper cuts”)

| Task | Manual steps today |
|---|---|
| **Define a message** | Write `#[protein]` enum/struct **and** duplicate it in every consumer cell; no central crate yet. |
| **Call another cell** | `let mut s = Synapse::grow("other").await?; let v = s.fire(my_msg).await?; let ans = rkyv::from_bytes::<Reply>(v.as_slice())?;` |
| **Register a handler** | `Membrane::bind("name", |v| async { … }).await` – you write the match/if-let chain yourself. |
| **Keep types in sync across lang** | Run `cell-bind` CLI **by hand** after every Rust change; commit generated files; foreign build may drift. |
| **Know what routes exist** | No LSP / autocomplete – you **read the source** or grep lock files. |
| **Clean zombies / rebuild** | `cell clean && cell mitosis .` – two commands, no watch mode. |
| **Dependency order** | You start cells **bottom-up** or let them crash-retry; no topological launcher yet. |
| **Hot-reload shaders** | Edit `.wgsl`, then **press “Force Reload” button** in ImGui – no file watcher. |
| **Billing locally** | Metrics are **in-memory only**; if you want invoices you opt-in with `Cell.toml` + Lightning adapter. |

---

### 3. Coming next (already planned / prototyped)

*   `#[cell::service]` macro → single `async fn` handler, no manual `Synapse::grow`, auto-generated client crate.  
*   `cell-bind lsp-bundle` → language-server package for Go/Python/TS with **real-time autocomplete** driven by the lock file.  
*   `cell mitosis . --watch` → one command that **watches source**, **kills zombies**, **rebuilds**, **re-spawns** and **hot-reloads** shaders.  
*   **Shared-memory bridge** for > 10 k entities (file-descriptor passing, zero copy).  
*   **Topological boot** – Stem sorts start order automatically.  

---

TL;DR  
The SDK **hides the nasty bits** (sandbox, crypto, zero-copy, schema hash) but **leaves you the fun parts** (write your logic, fire messages).  
The next polish layer will remove the remaining **manual glue** so you **never write `Synapse::grow` again** and get **IDE autocomplete** in every language.


---

Ranked by how much **real pain** they cause today vs. how much **real value** they add.

| # | Manual Task | Pain Level | Value Level | Verdict |
|---|-------------|------------|-------------|---------|
| 1 | **Duplicate message definitions in every consumer** | 🔥 **Agony** | 💡 **Critical** | **KILL** – central schema crate or code-gen must fix this |
| 2 | **Hand-written `Synapse::grow` + `rkyv` dance** | 🔥 **Agony** | 💡 **Critical** | **KILL** – one-line macro / client-gen already planned |
| 3 | **No autocomplete / discoverability** | 😤 **High** | 💡 **High** | **KILL** – LSP bundle gives instant Go/Python/Rust IntelliSense |
| 4 | **Two-step clean + mitosis** | 😤 **High** | 🚫 **Low** | **KILL** – merge into single `mitosis --watch` |
| 5 | **Foreign-language drift** | 😤 **High** | 💡 **High** | **KILL** – lock-file hash already exists, just enforce it in CI |
| 6 | **Manual `cell-bind` CLI invocation** | 😐 **Medium** | 💡 **Medium** | **KILL** – hook into `build.rs` so `cargo build` also spits out Go/Python |
| 7 | **No file-watcher for shaders** | 😐 **Medium** | 🎨 **Nice** | **KEEP** – low-hanging fruit, add `notify` crate + 30 lines |
| 8 | **Bottom-up start order** | 😐 **Medium** | 🚫 **Low** | **KEEP** – topological boot is easy, but not blocking anyone |
| 9 | **Local billing noise** | 🙄 **Low** | 🚫 **None** | **KILL** – metrics stay in RAM; billing adapter is **opt-in** only |
|10| **Zombie reaping** | 🙄 **Low** | 🛡️ **Medium** | **KEEP** – Stem already notices dead sockets; just add `prctl(PR_SET_PDEATHSIG)` |

---

Bottom line  
**Annoying** = anything you do **more than once per day** that **doesn’t teach you anything new**.  
The top **five red rows** are pure friction—eliminate them first.  
The rest are polish you can add once the **developer loop is invisible**.