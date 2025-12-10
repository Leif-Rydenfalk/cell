# Cell: Replace Your Datacenter With UDP Packets and Biology

I built a distributed OS where services replicate like bacteria and compute follows latency gradients automatically.

## The Actual Problem

Datacenters are feudalism:
- AWS owns the compute
- Docker Hub owns the registry  
- Kubernetes owns the scheduler
- DNS owns discovery
- Load balancers own routing

**Cost:** $$$$$$/month  
**Control:** Zero  
**Censorship resistance:** None

## The Cell Approach

Code is a **spore**. It floats through the network via DHT. When it lands on silicon with spare capacity, it **grows**.
```rust
// Write your service
#[cell::handler]
impl ExchangeService {
    async fn place_order(&self, symbol: String, amount: u64) -> Result<OrderId> { ... }
}

// Anyone, anywhere can use it
cell_remote!(Exchange = "exchange");
let mut client = Exchange::connect().await?;
```

**What just happened:**

1. Client queries DHT for "exchange" cells
2. Finds 47 instances across 3 continents
3. Measures latency to each via UDP probe
4. Tokyo client connects to Osaka instance (2ms)
5. Your laptop becomes seed #48 for that code

**No configuration. No YAML. No datacenter.**

## The Magic Parts

### 1. Auto-Scaling Via Mitosis
Cells monitor their own CPU. When stressed, they **spawn siblings**:
```toml
[cell.biology]
min_replicas = 3
cpu_target = 60%
```

Instance sees 85% CPU → probabilistic spawn → new replica auto-announces via pheromones

**No HPA, no metrics-server, no Prometheus scraping**

### 2. Git + DHT = Unkillable Registry
```toml
[cell]
exchange = { 
    authority = "ed25519:7a8f...",  # Author's pubkey
    mirrors = ["github.com/acme/exchange"]
}
```

Build script:
- Queries DHT for anyone with that signed code
- Downloads from **fastest peer** (not GitHub)  
- Your machine becomes mirror #N
- GitHub dies? Who cares, 10,000 other seeds exist

**Cost: $0. Bandwidth: Shared. Censorship: Impossible.**

### 3. Latency-Based Routing (Vivaldi Coordinates)

Every cell maintains a vector coordinate in "latency space":
```
Tokyo cell: [142.5, -23.1, 8.4]
NY cell: [-73.2, 40.7, 12.9]
Your laptop: [142.8, -23.3, 8.1]
```

Vector math instantly shows Tokyo cell is 2ms away, NY is 150ms

**No GeoIP database. No BGP. Just physics.**

### 4. Breaking Changes = New Species

Change `email: String` → `emails: Vec<String>`?

You didn't "upgrade" UserService. You created **UserServiceV2**.

Both run forever. Dependencies choose. Bad APIs become visible infrastructure debt.

**Evolutionary pressure for good design.**

## Performance

Same-machine (SHM): 1.5M msg/sec, 677ns RTT  
Same-LAN (QUIC): 50K msg/sec, 150µs RTT  
Cross-continent: Limited by speed of light

But that's not the point.

## The Point

**Every laptop, every Raspberry Pi, every VPS becomes a node in a global supercomputer that:**

- Discovers itself via UDP multicast
- Replicates code via DHT seeding
- Routes via latency measurements  
- Scales via autonomous cell division
- Survives datacenter outages
- Costs $0 to operate

You're not renting AWS. You're **growing** a compute organism.

## Current Status

Working:
- Local/LAN discovery
- Zero-copy SHM transport
- Compile-time schema validation
- Auto-spawn on high latency
- Process sandboxing

Missing:
- DHT implementation (using libp2p-kad)
- Git-over-QUIC transport
- Ed25519 signature verification
- Vivaldi coordinate system
- Production hardening

**Timeline:** 6-8 weeks to global mesh

## Why This Matters

If Cell works:

1. **Students** can horizontally scale across their campus LAN for $0
2. **Startups** can run planet-scale without AWS bills  
3. **Countries** can build censorship-resistant infrastructure
4. **Mars colonies** can run the same code with 20-minute latency

The substrate works the same whether nodes are:
- In the same rack (0.8µs via SHM)
- In the same city (150µs via LAN)
- On different planets (20min via deep space relay)

**Same API. Same binary. Same 20-byte header.**

## The Ask

I need help with:

1. **DHT architecture** - Kademlia vs custom
2. **Vivaldi stability** - Spring-mass convergence  
3. **Security model** - Ed25519 + reputation system
4. **Embedded targets** - Does no_std path work on your MCU?

[Repo](https://github.com/Leif-Rydenfalk/cell) | MIT License

---

**TL;DR:** I'm trying to make AWS optional by turning the internet into a biological organism. The RPC latency is just proof it doesn't suck.