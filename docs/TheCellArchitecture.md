# Cell System Documentation & Developer Reference

## 1. Philosophy: The Biological Substrate

Cell is a distributed computing substrate inspired by biological systems. Unlike traditional microservices (which are disparate binaries glued together by config files and HTTP), **Cells** are autonomous, stateful organisms that share a common genetic makeup and communication membrane.

*   **DNA (Source):** Source code is the single source of truth.
*   **Ribosome (Compiler):** The system compiles source code into executable proteins on-demand.
*   **Membrane (Transport):** Zero-copy, high-performance IPC (Unix Sockets/SHM) locally, upgrading to QUIC (Axon) for network travel.
*   **Organism (Scope):** A collection of cells working together to form a higher-order application.

---

## 2. System Architecture

The Cell System relies on a strict filesystem hierarchy to manage code, binaries, and runtime state. There is no central database; the filesystem is the database.

### 2.1. The Filesystem Hierarchy (`~/.cell`)

| Path | Purpose |
| :--- | :--- |
| `~/.cell/registry/` | **The Source of Truth.** Contains symlinks pointing to the source code of every known cell on the machine. Populated automatically by `cargo build`. |
| `~/.cell/proteins/` | **The Binary Cache.** Contains compiled binaries. Binaries are content-addressed (hashed based on source code). If the source hasn't changed, the existing binary is reused instantly. |
| `~/.cell/run/` | **Runtime State.** Contains Unix domain sockets and PID locks. |
| `~/.cell/run/global/` | **Kernel Space.** Sockets for system-wide infrastructure (Nucleus, Axon). |
| `~/.cell/run/<org_id>/` | **User Space.** Sockets for specific applications (Organisms) or test runs. |

### 2.2. Scope Resolution: Global vs. Organism

Cell implements a hierarchical scoping model similar to an Operating System's Kernel vs. User space.

1.  **Global Scope (`/global`):**
    *   Shared infrastructure that runs once per machine.
    *   Examples: **Nucleus** (System Manager), **Axon** (Network Gateway), **Autoscaler**.
    *   These cells connect the machine to the outside world and manage resources.

2.  **Organism Scope (`/<organism_id>`):**
    *   Application-specific namespaces.
    *   Examples: `ledger`, `engine`, `web-frontend`.
    *   You can run multiple instances of the "Market" application simultaneously (e.g., `prod`, `dev`, `test-run-1`) without port conflicts.

**Connection Logic:**
When a cell connects to another cell (e.g., `Synapse::grow("axon")`):
1.  It checks its **Local Organism** folder first.
2.  If not found, it checks the **Global** folder.
3.  This allows lightweight apps to plug into system-wide networking without spinning up their own stack.

---

## 3. Developer Workflow

### 3.1. Creating a Cell
A Cell is a Rust binary crate that uses `cell-sdk`.

```rust
use cell_sdk::*;

#[protein]
pub struct MyRequest { pub content: String }

#[service]
struct MyService;

#[handler]
impl MyService {
    async fn handle_req(&self, req: MyRequest) -> Result<String> {
        Ok(format!("Processed: {}", req.content))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let service = MyService;
    service.serve("my-cell").await
}
```

### 3.2. Registering (The `build.rs` Magic)
You do not "install" cells. You just build them.
Ensure your `build.rs` includes:

```rust
fn main() {
    cell_build::register(); 
}
```

When you run `cargo build`, the system detects the project path and creates a symlink in `~/.cell/registry/my-cell`. The System Daemon now knows this cell exists and how to compile it.

### 3.3. Spawning
You can spawn cells from the CLI or programmatically.

**CLI:**
```bash
# Spawns into the 'default' organism
cell spawn my-cell 

# Spawns into a specific organism
CELL_ORGANISM=dev-env cell spawn my-cell
```

**Programmatic (SDK):**
```rust
// Requests the Root Daemon to spawn the cell.
// The Root checks the Registry, compiles the binary if needed, and runs it.
System::spawn("my-cell", None).await?;
```

---

## 4. Testing Strategy

There are no "Test Mocks" or "Test Utils". Tests run on the real infrastructure using **Ephemeral Organisms**.

### 4.1. Integration Tests
When you run `cargo test`:
1.  The test uses `System::spawn`.
2.  The SDK detects it is running in a test context.
3.  It generates a random Organism ID (e.g., `test-7f8a9b`).
4.  It spawns the cells into `~/.cell/run/test-7f8a9b/`.
5.  The cells bind to private sockets in that folder.
6.  If the cells need networking (Axon), they connect to the **Real Global Axon** (saving boot time), or fallback if isolated.

**Example Test:**
```rust
#[tokio::test]
async fn test_market_flow() {
    // 1. Spawn the ensemble in a fresh organism scope
    System::spawn("ledger", None).await.unwrap();
    System::spawn("engine", None).await.unwrap();

    // 2. Connect (waits for socket to be ready)
    let mut engine = Engine::Client::connect().await.unwrap();

    // 3. Execute logic
    let result = engine.place_order(...).await.unwrap();
    assert!(result.is_ok());
}
```

---

## 5. Core Services (The Organs)

### **Root (The Hypervisor)**
*   **Role:** The daemon process managing the lifecycle of all cells.
*   **Responsibility:** Receives `MitosisRequest`, compiles code via `Ribosome`, launches processes, manages namespaces.

### **Nucleus (System Manager)**
*   **Role:** The brain.
*   **Responsibility:** Service Discovery, Health Monitoring, Configuration Management.
*   **Scope:** Global.

### **Axon (Network Gateway)**
*   **Role:** The nervous system.
*   **Responsibility:** Bridges local Unix Sockets to QUIC/TCP for inter-machine communication. Manages Pheromones (UDP Broadcast) for LAN discovery.
*   **Scope:** Global.

---

## 6. SDK Reference

### `System`
The interface to the Root Daemon.
*   `System::spawn(name, config)`: Request a new cell process.
*   `System::kill(name)`: Terminate a cell.

### `Synapse`
The neural connection between cells.
*   `Synapse::grow(name)`: Connect to a cell (resolves Local -> Global -> Network).
*   `Synapse::grow_await(name)`: Connect, polling until the socket appears (for spawning logic).

### `cell_remote!` (Macro)
Generates a typed RPC client for a remote cell based on its source code.
```rust
cell_remote!(Ledger = "ledger");
let mut client = Ledger::Client::connect().await?;
```