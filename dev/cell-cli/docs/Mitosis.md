# Mitosis: The Cellular Lifecycle & Dependency Resolution

The `cell mitosis` command is the bootloader for the Cell ecosystem. Its primary goal is to guarantee **Type Safety across the Network**.

Unlike standard RPC frameworks (gRPC, REST) which rely on potentially stale `.proto` or OpenAPI files, Cell uses a **Live Verification** strategy. You cannot compile a Cell unless its dependencies are physically running and their schemas have been verified against the current network reality.

---

## The Startup Flow (Algorithm)

When you run `cell mitosis .`, the CLI executes a **Recursive Topological Boot** (DAG Resolution).

### Phase 1: The Inventory
Before doing anything, the CLI recursively scans the current directory and all subdirectories to build a **Local Registry**.
*   It looks for `Cell.toml` files.
*   It maps `Cell Name` -> `File Path`.
*   *Result:* The CLI knows which cells *can* be built from source locally.

### Phase 2: Dependency Resolution (Recursive)
The CLI inspects the `[axons]` section of your `Cell.toml`. For every dependency (e.g., `worker`), it performs the following logic:

#### Step A: Network Discovery (Where is it?)
The CLI attempts to find the dependency's IP address using three strategies in order:

1.  **Direct Address:** If the config is `axon://192.168.1.50:9000`, it uses that IP immediately.
2.  **Pheromone Discovery (UDP):** If the config is a name (e.g., `axon://dad-worker`), the CLI opens a UDP socket and listens for Multicast Pheromones (Heartbeats) on the local network. If "dad-worker" is running on the LAN, it will be discovered.
3.  **Local Registry:** If the cell is not on the network, the CLI checks the **Inventory** (Phase 1). If the source code exists locally, it marks it for booting.

#### Step B: Liveness Check
*   **If found on Network:** The CLI attempts a TCP connection.
    *   *Success:* It proceeds to **Phase 3**.
    *   *Failure:* It falls back to booting from source (if available).
*   **If booting from Source:**
    *   The CLI pauses the current cell's startup.
    *   It **Recursively** calls `ensure_active()` on the dependency.
    *   This ensures the dependency *of the dependency* is running first.
    *   Once the dependency is compiled and spawned, it waits for the port to open.

### Phase 3: The Strict Snapshot (The "Handshake")
This is the critical safety step. We **never** use a cached schema without verification.

1.  **Secure Connect:** The CLI connects to the dependency (Local or Remote) using Noise Protocol (Curve25519).
2.  **Request Schema:** It sends a system signal `__GENOME__`.
3.  **Download:** The running dependency serializes its `input/output` types into JSON and sends them back.
4.  **Write:** The CLI writes this JSON to `.cell-genomes/{name}.json`.

> **Constraint:** If this connection fails, or the dependency returns a compilation error, **your build aborts**. This prevents "Works on my machine" bugs caused by stale API definitions.

### Phase 4: Protein Synthesis (Compilation)
Only now, with the schemas physically verified and present on disk, does `cargo build` run.
*   The `signal_receptor!` macros in your code read the `.cell-genomes/*.json` files.
*   Rust generates the types dynamically.
*   The binary is produced.

### Phase 5: Activation
The binary is spawned. It starts:
1.  **Nucleus:** The logic handler.
2.  **Golgi:** The router, which begins broadcasting Pheromones so *other* cells can find *it*.

---

## Scenario Examples

### Scenario A: The Monorepo (Zero to Hero)
You have a `coordinator` and a `worker` in the same folder. Nothing is running.
1.  You run `cell mitosis coordinator`.
2.  CLI sees `worker` in `[axons]`.
3.  CLI scans network -> Worker not found.
4.  CLI checks Inventory -> Found `worker` source code.
5.  **Recursion:** CLI switches focus to `worker`.
    *   Worker has no dependencies.
    *   Worker compiles.
    *   Worker spawns and binds port 9000.
6.  CLI connects to `localhost:9000`, downloads `worker.json`.
7.  Coordinator compiles (linking against `worker.json`).
8.  Coordinator spawns.

### Scenario B: The "Dad's Worker" (Hybrid Network)
You are building `my-cell`. Your `Cell.toml` has `dad = "axon://dads-pc"`. You do **not** have the source code for `dads-pc`.
1.  You run `cell mitosis my-cell`.
2.  CLI sees `dad` dependency.
3.  CLI listens on UDP (Pheromones).
4.  Dad's computer (on the LAN) broadcasts: *"I am dads-pc at 192.168.1.55:4000"*.
5.  CLI connects to `192.168.1.55:4000`.
6.  CLI downloads the schema for `dad`.
7.  `my-cell` compiles using the exact types defined on Dad's running machine.

### Scenario C: The Broken Chain
Your `Cell.toml` depends on `auth-service`.
1.  `auth-service` is not running on the network.
2.  `auth-service` source code is not in your directory.
3.  **Result:** `Mitosis Failed: CRITICAL: Dependency 'auth-service' is missing from network AND local workspace.`

---

## Visual Flowchart

```mermaid
graph TD
    Start[cell mitosis .] --> Inventory[Scan Recursive Inventory]
    Inventory --> ReadGenome[Read Cell.toml]
    ReadGenome --> CheckDeps{Has Axons?}
    
    CheckDeps -- Yes --> LoopDeps[For Each Dependency]
    CheckDeps -- No --> Genesis
    
    LoopDeps --> Resolve{Resolve Addr}
    Resolve -- IP defined --> VerifyConn
    Resolve -- Name defined --> Pheromones[Listen UDP Multicast]
    
    Pheromones -- Found --> VerifyConn[Verify TCP]
    Pheromones -- Not Found --> CheckLocal{In Inventory?}
    
    VerifyConn -- Success --> Snapshot[Snapshot Schema]
    VerifyConn -- Fail --> CheckLocal
    
    CheckLocal -- Yes --> Recurse[RECURSIVE BOOT]
    Recurse --> Boot[Compile & Spawn Dep]
    Boot --> VerifyConn
    
    CheckLocal -- No --> Error[CRITICAL FAILURE]
    
    Snapshot --> Genesis[Run Genesis (Local Codegen)]
    Genesis --> Compile[Cargo Build]
    Compile --> Spawn[Spawn Cell]
    Spawn --> Golgi[Start Golgi Router]
```

Because `mitosis` runs **before** `cargo build`, the types defined by the remote cell exist on your disk as if they were local code before the compiler ever runs.

Here is exactly how the **Compile-Time Safety Net** catches a mismatch:

### The "Chain of Verification"

1.  **The Snapshot (Mitosis):**
    The CLI connects to "Dad's Worker" and downloads `worker.json`.
    ```json
    // .cell/data/worker.json
    {
      "input": "ProcessImageRequest",
      "output": "ImageResult"
    }
    ```

2.  **The Code Gen (Macro Expansion):**
    When you run `cargo build`, your code contains a macro that reads that JSON file.
    ```rust
    // src/main.rs
    // The macro reads ".cell/data/worker.json" at compile time
    #[cell::import(service = "worker")] 
    struct WorkerClient;
    ```
    
    The **Compiler** (via the macro) invisibly generates this Rust code:
    ```rust
    // GENERATED CODE (Invisible to you, but seen by rustc)
    impl CellClient for WorkerClient {
        type Input = ProcessImageRequest;  // Enforced from JSON
        type Output = ImageResult;         // Enforced from JSON
    }

    impl WorkerClient {
        pub async fn send(req: ProcessImageRequest) -> ImageResult { ... }
    }
    ```

3.  **The Compiler Check (Rustc):**
    Now, imagine you try to send the wrong data type in your code:

    ```rust
    let client = WorkerClient::new();
    
    // ERROR!
    // You are trying to send a String, but the downloaded schema 
    // proves that 'worker' demands a 'ProcessImageRequest'.
    client.send("some_string".to_string()).await; 
    ```

### The Result
The build will **fail instantly** with a standard Rust type error:

```text
error[E0308]: mismatched types
  --> src/main.rs:15:18
   |
15 |     client.send("some_string".to_string()).await;
   |            ----  ^^^^^^^^^^^^^^^^^^^^^^^ expected struct `ProcessImageRequest`, found struct `String`
   |            |
   |            arguments to this function are incorrect
   |
   = note: expected struct `ProcessImageRequest`
              found struct `String`
```

### Summary
You cannot deploy code that talks to a service that doesn't exist, nor can you send data that the service isn't explicitly typed to receive. The network reality is baked into your binary at build time.