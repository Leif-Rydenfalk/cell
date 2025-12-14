# YES. Load Balancing is INVISIBLE to the developer.

## The Magic: One Name, Many Instances

```rust
// You write this:
let mut trading = cell_remote!(trading = "france-trading");
let price = trading.get_current_price("BTC".into()).await?;

// Behind the scenes:
// - "france-trading" resolves to 100 instances across the globe
// - System picks the fastest/closest one automatically
// - If it fails, instantly tries another
// - You never know, never care
```

## Implementation: The Discovery Layer

### Step 1: Cell Registration (Automatic on Spawn)

```rust
// cell-sdk/src/membrane.rs

impl Membrane {
    pub async fn bind<F, Fut>(name: &str, handler: F) -> Result<()>
    where
        F: Fn(Vesicle) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Vesicle>> + Send,
    {
        // ... existing bind logic ...
        
        // NEW: Register with discovery system
        let discovery = DiscoveryClient::global().await?;
        
        // Announce ourselves
        discovery.register(Registration {
            cell_name: name.to_string(),
            fingerprint: CELL_GENOME_FINGERPRINT,
            endpoint: Endpoint::Local(socket_path.clone()),
            region: detect_region(),
            load: 0.0,
            health: Health::Up,
        }).await?;
        
        // Heartbeat loop
        let disc = discovery.clone();
        let cell_name = name.to_string();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                disc.heartbeat(&cell_name).await.ok();
            }
        });
        
        // ... rest of bind logic ...
    }
}
```

### Step 2: Discovery Protocol (Gossip-Based)

```rust
// cell-sdk/src/discovery.rs

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct DiscoveryClient {
    // Local knowledge of all cells
    registry: Arc<RwLock<HashMap<String, Vec<CellInstance>>>>,
    
    // Gossip protocol for P2P sync
    gossip: GossipEngine,
    
    // Performance metrics
    latency_tracker: LatencyTracker,
}

#[derive(Clone, Debug)]
pub struct CellInstance {
    pub cell_name: String,
    pub fingerprint: u64,
    pub endpoint: Endpoint,
    pub region: String,
    pub load: f32,           // 0.0 to 1.0
    pub health: Health,
    pub last_seen: Instant,
    pub avg_latency: Duration, // Measured by us
}

#[derive(Clone, Debug)]
pub enum Endpoint {
    Local(PathBuf),                    // Unix socket
    Remote { host: String, port: u16 }, // TCP/QUIC
    Relay(Box<Endpoint>),              // NAT traversal
}

#[derive(Clone, Debug)]
pub enum Health {
    Up,
    Degraded,
    Down,
}

impl DiscoveryClient {
    pub async fn global() -> Result<Arc<Self>> {
        // Singleton instance
        static INSTANCE: OnceCell<Arc<DiscoveryClient>> = OnceCell::new();
        
        INSTANCE.get_or_try_init(|| async {
            let client = Self {
                registry: Arc::new(RwLock::new(HashMap::new())),
                gossip: GossipEngine::new().await?,
                latency_tracker: LatencyTracker::new(),
            };
            
            // Start gossip listener
            client.start_gossip_listener();
            
            Ok(Arc::new(client))
        }).await.cloned()
    }
    
    pub async fn register(&self, reg: Registration) -> Result<()> {
        let instance = CellInstance {
            cell_name: reg.cell_name.clone(),
            fingerprint: reg.fingerprint,
            endpoint: reg.endpoint.clone(),
            region: reg.region,
            load: reg.load,
            health: reg.health,
            last_seen: Instant::now(),
            avg_latency: Duration::from_millis(0),
        };
        
        // Add to local registry
        let mut registry = self.registry.write().await;
        registry.entry(reg.cell_name.clone())
            .or_insert_with(Vec::new)
            .push(instance.clone());
        
        // Gossip to peers
        self.gossip.announce(instance).await?;
        
        Ok(())
    }
    
    pub async fn discover(&self, cell_name: &str) -> Result<Vec<CellInstance>> {
        // Check local registry first
        {
            let registry = self.registry.read().await;
            if let Some(instances) = registry.get(cell_name) {
                if !instances.is_empty() {
                    return Ok(instances.clone());
                }
            }
        }
        
        // Query gossip network
        let instances = self.gossip.query(cell_name).await?;
        
        // Update local cache
        if !instances.is_empty() {
            let mut registry = self.registry.write().await;
            registry.insert(cell_name.to_string(), instances.clone());
        }
        
        Ok(instances)
    }
    
    pub async fn heartbeat(&self, cell_name: &str) -> Result<()> {
        let mut registry = self.registry.write().await;
        if let Some(instances) = registry.get_mut(cell_name) {
            for instance in instances.iter_mut() {
                if matches!(instance.endpoint, Endpoint::Local(_)) {
                    instance.last_seen = Instant::now();
                    instance.health = Health::Up;
                }
            }
        }
        Ok(())
    }
    
    fn start_gossip_listener(&self) {
        let registry = self.registry.clone();
        let gossip = self.gossip.clone();
        
        tokio::spawn(async move {
            loop {
                // Receive gossip messages
                if let Ok(msg) = gossip.recv().await {
                    match msg {
                        GossipMessage::Announce(instance) => {
                            let mut reg = registry.write().await;
                            reg.entry(instance.cell_name.clone())
                                .or_insert_with(Vec::new)
                                .push(instance);
                        }
                        GossipMessage::Heartbeat { cell_name, endpoint } => {
                            let mut reg = registry.write().await;
                            if let Some(instances) = reg.get_mut(&cell_name) {
                                for inst in instances.iter_mut() {
                                    if inst.endpoint == endpoint {
                                        inst.last_seen = Instant::now();
                                        inst.health = Health::Up;
                                    }
                                }
                            }
                        }
                        GossipMessage::Remove { cell_name, endpoint } => {
                            let mut reg = registry.write().await;
                            if let Some(instances) = reg.get_mut(&cell_name) {
                                instances.retain(|i| i.endpoint != endpoint);
                            }
                        }
                    }
                }
            }
        });
    }
}

// Simple gossip protocol
pub struct GossipEngine {
    peers: Arc<RwLock<Vec<SocketAddr>>>,
    socket: Arc<UdpSocket>,
}

impl GossipEngine {
    pub async fn new() -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        
        // Join multicast group for local discovery
        let multicast_addr = Ipv4Addr::new(239, 255, 42, 1);
        socket.join_multicast_v4(multicast_addr, Ipv4Addr::UNSPECIFIED)?;
        
        Ok(Self {
            peers: Arc::new(RwLock::new(Vec::new())),
            socket: Arc::new(socket),
        })
    }
    
    pub async fn announce(&self, instance: CellInstance) -> Result<()> {
        let msg = GossipMessage::Announce(instance);
        let bytes = serde_json::to_vec(&msg)?;
        
        // Multicast to local network
        self.socket.send_to(&bytes, "239.255.42.1:9042").await?;
        
        // Unicast to known peers
        let peers = self.peers.read().await;
        for peer in peers.iter() {
            self.socket.send_to(&bytes, peer).await.ok();
        }
        
        Ok(())
    }
    
    pub async fn query(&self, cell_name: &str) -> Result<Vec<CellInstance>> {
        // Send query via gossip
        let msg = GossipMessage::Query { cell_name: cell_name.to_string() };
        let bytes = serde_json::to_vec(&msg)?;
        
        self.socket.send_to(&bytes, "239.255.42.1:9042").await?;
        
        // Wait for responses (with timeout)
        let mut instances = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(500);
        
        while Instant::now() < deadline {
            let mut buf = vec![0u8; 65536];
            match tokio::time::timeout(
                deadline - Instant::now(),
                self.socket.recv_from(&mut buf)
            ).await {
                Ok(Ok((len, _))) => {
                    if let Ok(GossipMessage::QueryResponse { instances: insts }) = 
                        serde_json::from_slice(&buf[..len]) {
                        instances.extend(insts);
                    }
                }
                _ => break,
            }
        }
        
        Ok(instances)
    }
}

#[derive(Serialize, Deserialize)]
enum GossipMessage {
    Announce(CellInstance),
    Heartbeat { cell_name: String, endpoint: Endpoint },
    Remove { cell_name: String, endpoint: Endpoint },
    Query { cell_name: String },
    QueryResponse { instances: Vec<CellInstance> },
}
```

### Step 3: Smart Routing (The Magic)

```rust
// cell-sdk/src/synapse.rs

pub struct Synapse {
    cell_name: String,
    instances: Vec<CellInstance>,
    discovery: Arc<DiscoveryClient>,
    router: SmartRouter,
}

pub struct SmartRouter {
    strategy: RoutingStrategy,
    circuit_breakers: HashMap<Endpoint, CircuitBreaker>,
}

pub enum RoutingStrategy {
    Fastest,           // Lowest latency
    RoundRobin,        // Simple rotation
    LeastLoaded,       // Lowest CPU/memory
    Geographic,        // Same region preferred
    Adaptive,          // ML-based prediction
}

impl Synapse {
    pub async fn grow(cell_name: &str) -> Result<Self> {
        let discovery = DiscoveryClient::global().await?;
        
        // Discover all instances
        let instances = discovery.discover(cell_name).await?;
        
        if instances.is_empty() {
            // Try to spawn via mitosis
            trigger_mitosis(cell_name).await?;
            
            // Retry discovery
            let instances = discovery.discover(cell_name).await?;
            if instances.is_empty() {
                bail!("No instances found for '{}'", cell_name);
            }
        }
        
        Ok(Self {
            cell_name: cell_name.to_string(),
            instances,
            discovery,
            router: SmartRouter::new(RoutingStrategy::Adaptive),
        })
    }
    
    pub async fn fire<T: Protein>(&mut self, msg: T) -> Result<T::Response> {
        // Refresh instances periodically
        if self.should_refresh() {
            self.instances = self.discovery.discover(&self.cell_name).await?;
        }
        
        // Filter healthy instances
        let healthy: Vec<_> = self.instances.iter()
            .filter(|i| i.health == Health::Up)
            .filter(|i| !self.router.is_open(&i.endpoint))
            .collect();
        
        if healthy.is_empty() {
            bail!("No healthy instances available");
        }
        
        // Pick best instance
        let target = self.router.select(&healthy)?;
        
        // Execute with retry on failure
        for attempt in 0..3 {
            match self.try_call(target, &msg).await {
                Ok(response) => {
                    // Record success
                    self.router.record_success(&target.endpoint);
                    return Ok(response);
                }
                Err(e) if attempt < 2 => {
                    // Record failure
                    self.router.record_failure(&target.endpoint);
                    
                    // Try next best instance
                    if let Some(fallback) = self.router.select(&healthy).ok() {
                        target = fallback;
                        continue;
                    }
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
        }
        
        bail!("All retries exhausted")
    }
    
    async fn try_call<T: Protein>(
        &mut self,
        instance: &CellInstance,
        msg: &T,
    ) -> Result<T::Response> {
        let start = Instant::now();
        
        let mut conn = Connection::connect(&instance.endpoint).await?;
        let bytes = rkyv::to_bytes(msg)?.into_vec();
        
        conn.send(bytes).await?;
        let resp_bytes = conn.recv().await?;
        
        let response = rkyv::from_bytes(&resp_bytes)?;
        
        // Record latency
        let latency = start.elapsed();
        self.discovery.latency_tracker.record(
            &instance.endpoint,
            latency,
        ).await;
        
        Ok(response)
    }
}

impl SmartRouter {
    pub fn select<'a>(&self, instances: &[&'a CellInstance]) -> Result<&'a CellInstance> {
        match self.strategy {
            RoutingStrategy::Fastest => {
                // Pick instance with lowest average latency
                instances.iter()
                    .min_by_key(|i| i.avg_latency)
                    .copied()
                    .ok_or_else(|| anyhow!("No instances"))
            }
            
            RoutingStrategy::LeastLoaded => {
                // Pick instance with lowest load
                instances.iter()
                    .min_by(|a, b| a.load.partial_cmp(&b.load).unwrap())
                    .copied()
                    .ok_or_else(|| anyhow!("No instances"))
            }
            
            RoutingStrategy::Geographic => {
                // Prefer same region
                let my_region = detect_region();
                
                // First try same region
                if let Some(local) = instances.iter()
                    .find(|i| i.region == my_region) {
                    return Ok(local);
                }
                
                // Fallback to closest
                instances.iter()
                    .min_by_key(|i| i.avg_latency)
                    .copied()
                    .ok_or_else(|| anyhow!("No instances"))
            }
            
            RoutingStrategy::Adaptive => {
                // Weighted random based on performance
                let total_score: f32 = instances.iter()
                    .map(|i| self.calculate_score(i))
                    .sum();
                
                let mut rng = rand::thread_rng();
                let mut pick = rng.gen::<f32>() * total_score;
                
                for instance in instances {
                    let score = self.calculate_score(instance);
                    if pick < score {
                        return Ok(instance);
                    }
                    pick -= score;
                }
                
                // Fallback
                Ok(instances[0])
            }
            
            RoutingStrategy::RoundRobin => {
                // Simple rotation (stateful)
                todo!()
            }
        }
    }
    
    fn calculate_score(&self, instance: &CellInstance) -> f32 {
        // Higher score = better instance
        let latency_score = 1000.0 / (instance.avg_latency.as_millis() as f32 + 1.0);
        let load_score = 1.0 - instance.load;
        let health_score = match instance.health {
            Health::Up => 1.0,
            Health::Degraded => 0.5,
            Health::Down => 0.0,
        };
        
        latency_score * load_score * health_score
    }
    
    fn is_open(&self, endpoint: &Endpoint) -> bool {
        self.circuit_breakers
            .get(endpoint)
            .map(|cb| cb.is_open())
            .unwrap_or(false)
    }
    
    fn record_success(&mut self, endpoint: &Endpoint) {
        self.circuit_breakers
            .entry(endpoint.clone())
            .or_insert_with(CircuitBreaker::new)
            .record_success();
    }
    
    fn record_failure(&mut self, endpoint: &Endpoint) {
        self.circuit_breakers
            .entry(endpoint.clone())
            .or_insert_with(CircuitBreaker::new)
            .record_failure();
    }
}

// Simple circuit breaker
pub struct CircuitBreaker {
    failures: u32,
    state: BreakerState,
    last_failure: Option<Instant>,
}

enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            failures: 0,
            state: BreakerState::Closed,
            last_failure: None,
        }
    }
    
    fn is_open(&self) -> bool {
        match self.state {
            BreakerState::Open => {
                // Auto-recover after 30s
                if let Some(last) = self.last_failure {
                    if last.elapsed() < Duration::from_secs(30) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }
    
    fn record_success(&mut self) {
        self.failures = 0;
        self.state = BreakerState::Closed;
    }
    
    fn record_failure(&mut self) {
        self.failures += 1;
        self.last_failure = Some(Instant::now());
        
        if self.failures >= 5 {
            self.state = BreakerState::Open;
        }
    }
}
```

### Step 4: The Developer Experience

```rust
// You write this simple code:
use cell::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Single line - system handles everything
    let mut trading = cell_remote!(trading = "france-trading");
    
    // This call:
    // 1. Discovers 100 instances globally
    // 2. Measures latency to each
    // 3. Picks the fastest one
    // 4. Sends request
    // 5. If it fails, instantly retries with next best
    // 6. Tracks performance for next call
    
    let price = trading.get_current_price("BTC".into()).await?;
    println!("BTC: ${}", price.value);
    
    Ok(())
}
```

### Behind the scenes:

```
You (Sweden) call trading.get_current_price()
    ↓
System discovers instances:
    - france-trading@paris.fr (latency: 25ms, load: 0.2)
    - france-trading@london.uk (latency: 15ms, load: 0.5)
    - france-trading@stockholm.se (latency: 2ms, load: 0.8)  ← PICKS THIS
    - france-trading@tokyo.jp (latency: 180ms, load: 0.1)
    - ... 96 more instances ...
    ↓
Connects to stockholm.se (closest, fastest)
    ↓
Gets response in 2ms
    ↓
Records: "stockholm.se is fast, use it next time"
```

## Automatic Scaling

```rust
// Root monitors load and spawns more instances automatically

impl MyceliumRoot {
    async fn monitor_load(&self) {
        loop {
            sleep(Duration::from_secs(10)).await;
            
            let discovery = DiscoveryClient::global().await?;
            let all_cells = discovery.registry.read().await;
            
            for (cell_name, instances) in all_cells.iter() {
                let avg_load: f32 = instances.iter()
                    .map(|i| i.load)
                    .sum::<f32>() / instances.len() as f32;
                
                // Scale up if overloaded
                if avg_load > 0.8 && instances.len() < 10 {
                    println!("[Root] Scaling up '{}' (load: {:.2})", cell_name, avg_load);
                    self.spawn_cell(cell_name).await?;
                }
                
                // Scale down if underutilized
                if avg_load < 0.2 && instances.len() > 1 {
                    println!("[Root] Scaling down '{}' (load: {:.2})", cell_name, avg_load);
                    self.kill_slowest_instance(cell_name, &instances).await?;
                }
            }
        }
    }
}
```

## The Result

```rust
// Developer writes this:
let mut trading = cell_remote!(trading = "france-trading");
let price = trading.get_price("BTC".into()).await?;

// System does this:
// 1. Discovers 100 instances across 20 countries
// 2. Picks closest/fastest automatically
// 3. Retries on failure
// 4. Load balances future requests
// 5. Auto-scales based on demand
// 6. Circuit-breaks bad instances
// 7. Tracks performance metrics
// 8. All transparent to developer
```

**The developer never knows there are 100 instances. They just call one name. The system handles everything.**

This is the dream. This is Cell. **Go build it.**