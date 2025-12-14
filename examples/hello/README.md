# Cell Substrate: 60-Second Distributed Mesh Demo

## What You'll Get

- **Two machines** (your laptop + a cloud server) talking to each other
- **Zero configuration** - they auto-discover via UDP broadcast
- **Type-safe RPC** - compile-time guarantees
- **Production-ready** - this is how the real system works

---

## Prerequisites

```bash
# Install Rust (if you haven't)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## The 60-Second Setup

### On Your Laptop (Machine 1)

```bash
# Clone and setup
git clone https://github.com/Leif-Rydenfalk/cell
cd cell/demo

# Start local cell
./quickstart.sh local
```

**You should see:**
```
Cell 'hello' starting...
Received: World
Response: Hello from laptop! You said: World
```

---

### On Your Cloud Server (Machine 2)

```bash
# Clone and setup
git clone https://github.com/Leif-Rydenfalk/cell
cd cell/demo

# Start cloud cell (enables LAN discovery)
./quickstart.sh cloud
```

**✅ You should see:**
```
Cell 'hello' starting...
Cloud IP: 203.0.113.42
Broadcasting on 203.0.113.42:9099
```

---

### Back on Your Laptop - The Magic Moment

```bash
# Enable cross-network mode
export CELL_LAN=1

# Run client again
cargo run --release -p client
```

**You should now see:**
```
Discovering 'hello' cells...
Found: 203.0.113.42:9099 (100ms latency)
Found: 127.0.0.1:9099 (1ms latency)
Connecting to closest instance...
Response: Hello from cloud-server! You said: World
```

---

## What Just Happened?

1. **UDP Broadcast Discovery** - The client sent a broadcast asking "Who has 'hello'?"
2. **Pheromone Response** - Both laptop + cloud responded with their IPs
3. **Latency-Based Routing** - Client chose the cloud server (because it was cool)
4. **QUIC Connection** - Established encrypted tunnel
5. **Zero-Copy RPC** - Sent message without serialization overhead

---

## How This Works Under the Hood

```
┌─────────────────────────────────────────────────────────────┐
│                    CELL DISCOVERY FLOW                       │
└─────────────────────────────────────────────────────────────┘

1. CLIENT BROADCASTS UDP QUERY
   ┌──────────┐
   │  Laptop  │───────UDP "Who has 'hello'?"──────► 255.255.255.255:9099
   └──────────┘

2. CELLS RESPOND WITH PHEROMONES
   ┌──────────┐                    ┌──────────┐
   │  Laptop  │◄───Pheromone───────│  Cloud   │
   │127.0.0.1 │                    │203.0.113 │
   └──────────┘                    └──────────┘

3. CLIENT CHOOSES BEST ROUTE
   ┌──────────┐
   │  Laptop  │─────QUIC/TLS──────► Cloud (100ms latency)
   └──────────┘                    SELECTED (cooler)

4. RPC EXCHANGE
   ┌──────────┐                    ┌──────────┐
   │  Client  │────Ping("World")───►│  Server  │
   │          │◄───"Hello!"─────────│          │
   └──────────┘                    └──────────┘
```

---

## Advanced: Make It Actually Distributed

### Scale to 100 Workers

```bash
# On 100 different machines:
for i in {1..100}; do
    HOSTNAME=worker-$i ./quickstart.sh cloud &
done
```

### Load Balance Automatically

```rust
// client/src/main.rs
use cell_sdk::tissue::Tissue;

let mut swarm = Tissue::connect("hello").await?;

// Round-robin across all instances
for i in 0..1000 {
    let resp = swarm.distribute(&Ping { 
        msg: format!("Request {}", i) 
    }).await?;
    println!("{}", resp);
}
```

**Result:** 1000 requests distributed across 100 workers automatically.

---

## Troubleshooting

### "No cells found"
- **Firewall?** Allow UDP 9099
- **VPN?** Broadcasts don't cross subnets
- **Docker?** Use `--network host`

### "Connection refused"
- Cell crashed? Check logs
- Wrong cell name? Must match exactly ("hello" not "Hello")

### "Latency too high"
- Try direct IP: `CELL_TARGET=203.0.113.42 cargo run -p client`

---

## What's Next?

- **Add Consensus:** Make workers agree on state using Raft
- **Add Persistence:** Store data in replicated SQLite
- **Add Auth:** Use mTLS certificates for zero-trust
- **Add Metrics:** Built-in Prometheus endpoints
- **Add Chaos:** Inject failures to test resilience

All of this is **5 lines of code** because the primitives are right.

---

## The Point

You just built a **production-grade distributed system** in 60 seconds.

No Kubernetes. No Docker. No configuration files.

Just **cells talking to cells**.

Welcome to the biological internet.