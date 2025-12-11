# Cell Infrastructure - Advanced Features

## Architecture Overview

The Cell framework now uses a **cell-based management** architecture where system functions are delegated to specialized infrastructure cells:

```
┌─────────────────────────────────────────────────────────┐
│                      User Cells                         │
│              (Your Application Code)                    │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────────┐
│                   Cell SDK (Minimal)                    │
│          • RPC Primitives                               │
│          • Macros (#[handler], #[protein])             │
│          • Transport Layer                              │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────────┐
│              Infrastructure Cells                       │
│  ┌──────────┬──────────┬──────────┬──────────┐        │
│  │ Nucleus  │   DHT    │ Autoscal │ Vivaldi  │        │
│  ├──────────┼──────────┼──────────┼──────────┤        │
│  │ Registry │ Codegen  │ Consensus│   ...    │        │
│  └──────────┴──────────┴──────────┴──────────┘        │
└─────────────────────────────────────────────────────────┘
```

## Infrastructure Cells

### 1. **Nucleus** - System Manager
**Purpose:** Single-instance system orchestrator that manages cell lifecycle

**Responsibilities:**
- Cell registration and heartbeat monitoring
- Instance tracking
- Discovery coordinator
- Health aggregation

**Usage:**
```bash
# Start nucleus (automatically becomes singleton)
cell nucleus

# Register your cell
let mut nucleus = NucleusClient::connect().await?;
nucleus.register("my-cell".to_string(), node_id).await?;
```

### 2. **DHT** - Distributed Hash Table
**Purpose:** Global WAN-scale service discovery using Kademlia

**Features:**
- 160-bit key space (SHA-1)
- K=20 replication factor
- Iterative routing
- O(log N) lookup complexity

**Usage:**
```bash
# Store data globally
cell dht put my-key "my-value"

# Retrieve
cell dht get my-key

# Stats
cell dht stats
```

**API:**
```rust
cell_remote!(dht = "dht");

let mut client = dht::connect().await?;
client.store(dht::DhtStore {
    key: "service:my-cell".to_string(),
    value: "192.168.1.100:9000".as_bytes().to_vec(),
    ttl_secs: 3600,
}).await?;
```

### 3. **Autoscaler** - Mitosis/Apoptosis
**Purpose:** Autonomous scaling based on metrics

**Triggers:**
- CPU utilization
- Memory pressure
- Request latency (P99)
- Custom metrics

**Usage:**
```bash
# Define scaling policy
cell_remote!(autoscaler = "autoscaler");

let mut client = autoscaler::connect().await?;
client.register_policy(autoscaler::ScalingPolicy {
    cell_name: "my-service".to_string(),
    min_instances: 2,
    max_instances: 20,
    target_cpu: 70.0,
    target_memory_mb: 512,
    target_latency_ms: 100,
    cooldown_secs: 60,
}).await?;

# Manual scaling
cell scale my-service 5
```

### 4. **Vivaldi** - Network Coordinates
**Purpose:** Latency-aware routing using synthetic coordinates

**Algorithm:**
- Nodes converge to positions where Euclidean distance = RTT
- Spring-mass physics simulation
- Error-weighted updates

**Usage:**
```rust
cell_remote!(vivaldi = "vivaldi");

let mut client = vivaldi::connect().await?;

// Get best instances by latency
let result = client.route(vivaldi::RoutingQuery {
    target_cell: "my-service".to_string(),
    source_coordinate: my_coord,
    max_results: 3,
}).await?;

// Connect to closest instance
let closest = result.instances.first().unwrap();
```

### 5. **Registry** - Git-based Package Manager
**Purpose:** Decentralized package registry with signature verification

**Features:**
- Git as storage (no central server)
- Ed25519 signature verification
- Reproducible builds
- Transitive dependencies

**Usage:**
```bash
# Search
cell registry search "network"

# Install
cell registry install cell-http-server

# Publish (signs with your key)
cell registry publish ./my-cell

# Trust a developer
cell registry trust alice <public-key>
```

### 6. **Codegen** - Polyglot Bindings
**Purpose:** Generate clients in Python, Go, TypeScript, etc.

**Supported Languages:**
- Python (msgpack + Unix sockets)
- Go (binary encoding)
- TypeScript (Node.js client)
- Rust (native)
- Java, C (coming soon)

**Usage:**
```bash
# Generate Python client
cell generate my-service --language python --output my_service.py

# Use generated client
python3 << EOF
from my_service import MyServiceClient

client = MyServiceClient()
client.connect()
result = client.my_method(arg1="value")
print(result)
EOF
```

### 7. **Consensus** - Raft Cluster
**Purpose:** Strongly consistent distributed state

**Features:**
- Leader election
- Log replication
- Auto-discovery via Nucleus
- Snapshot support

**Usage:**
```rust
cell_remote!(consensus = "consensus");

let mut client = consensus::connect().await?;

// Write (only leader accepts)
client.write(consensus::ConsensusWrite {
    key: "counter".to_string(),
    value: b"42".to_vec(),
}).await?;

// Read (any node)
let value = client.read(consensus::ConsensusQuery {
    key: "counter".to_string(),
}).await?;
```

## Macro Coordination

Cells can now coordinate at compile-time using the `#[expand]` macro:

```rust
// cell-a/src/main.rs
use cell_sdk::*;

#[cell_macro]
fn add_field_macro(input: TokenStream) -> TokenStream {
    // Generate code that adds a field
    quote! {
        pub extra_field: u64
    }.into()
}

// cell-b/src/main.rs (depends on cell-a)
#[expand(cell_a, add_field_macro)]
#[protein]
pub struct MyStruct {
    pub name: String,
    // `extra_field` added by cell-a's macro
}
```

This enables:
- Cross-cell code generation
- Distributed metaprogramming
- Type-safe protocol evolution

## Complete Setup Guide

### 1. Initialize System
```bash
# Install CLI
cargo install --path cell-cli

# Initialize node
cell init --node-id 1

# Start infrastructure
cell nucleus &
cell deploy dht &
cell deploy autoscaler &
cell deploy vivaldi &
cell deploy registry &
cell deploy consensus --instances 3 &
```

### 2. Deploy Your Cell
```bash
# Create cell
cargo new --bin my-service
cd my-service

# Add dependency
echo 'cell-sdk = { path = "../cell-sdk" }' >> Cargo.toml

# Write service
cat > src/main.rs << 'EOF'
use cell_sdk::*;

#[protein]
pub struct Request {
    pub name: String,
}

#[protein]
pub struct Response {
    pub greeting: String,
}

pub struct MyService;

#[handler]
impl MyService {
    pub async fn greet(&self, req: Request) -> Result<Response> {
        Ok(Response {
            greeting: format!("Hello, {}!", req.name),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    MyService.serve("my-service").await
}
EOF

# Build and deploy
cargo build --release
cell deploy my-service --instances 3
```

### 3. Call from Client
```rust
use cell_sdk::*;

cell_remote!(my_service = "my-service");

#[tokio::main]
async fn main() -> Result<()> {
    let mut client = my_service::connect().await?;
    let response = client.greet(my_service::Request {
        name: "World".to_string(),
    }).await?;
    
    println!("{}", response.greeting);
    Ok(())
}
```

### 4. Generate Python Client
```bash
cell generate my-service --language python --output my_service.py

python3 << 'EOF'
from my_service import MyServiceClient

client = MyServiceClient()
client.connect()
result = client.greet(name="Python")
print(result['greeting'])
EOF
```

## Monitoring & Operations

```bash
# Check health
cell health

# View topology
cell topology

# Inspect cluster
cell_remote!(consensus = "consensus");
let status = consensus::connect().await?.status().await?;

# Scale based on load
cell scale my-service 10
```

## Advanced: Cross-Language Macro Coordination

```rust
// Python code generation cell
#[cell_macro]
pub fn generate_pydantic_model(struct_def: TokenStream) -> String {
    // Analyze Rust struct
    // Generate equivalent Pydantic model
    format!(r#"
from pydantic import BaseModel

class {name}(BaseModel):
    {fields}
"#)
}

// In another cell:
#[expand(codegen, generate_pydantic_model)]
#[protein]
pub struct User {
    pub id: u64,
    pub name: String,
}
// Generates: user.py with Pydantic model
```

## Performance Characteristics

| Feature | Latency | Throughput |
|---------|---------|------------|
| Local (SHM) | 2-5μs | 10M ops/s |
| LAN (QUIC) | 200μs | 500K ops/s |
| WAN (DHT) | 50-200ms | 10K ops/s |
| Consensus | 10-50ms | 1K writes/s |

## Security

- **Sandboxing:** Capsid uses bubblewrap for Linux namespace isolation
- **Signatures:** Registry verifies Ed25519 signatures
- **SHM Auth:** Challenge-response with process UID checks
- **Network:** mTLS for WAN (via QUIC)

## Next Steps

1. **Custom Cells:** Build domain-specific infrastructure
2. **Federation:** Connect multiple clusters via DHT
3. **Observability:** Integrate with Prometheus/Grafana
4. **Multi-Cloud:** Deploy across AWS/GCP/Azure

## Troubleshooting

```bash
# Check if nucleus is running
ps aux | grep cell-nucleus

# View logs
journalctl -u cell-nucleus -f

# Reset state
rm -rf ~/.cell/run/*
cell init

# Debug discovery
RUST_LOG=debug cell discover
```