# Implementation Plan: Global Cell Network (Revised)

Perfect. Crystal clear answers. Let me revise the plan with these constraints.

## Core Principles (Locked In)

1. **Cell-Git**: A cell-based git hosting service with instance metadata stored **outside** the git tree
2. **Schema Fingerprint**: Use existing `SCHEMA_FINGERPRINT` from `#[handler]` macro, no changes
3. **Binary Policy**: Always compile from source (no pre-compiled binary downloads for now)
4. **Network Policy**: Network is mandatory for remote deps. All cache expires in 60 seconds. If offline, you only get localhost cells.

---

## Phase 1: Cell-Git Service

### 1.1 The Cell-Git Architecture

Cell-Git is a **cell** that wraps git repositories and adds real-time instance tracking.

**Storage Layout:**
```
~/.cell/git/
├── repos/
│   └── {org}/
│       └── {name}/
│           ├── repo.git/           # Bare git repo (standard)
│           └── Cell.json           # Immutable manifest (tracked in git)
└── instances/
    └── {org}/{name}/
        └── instances.json          # Mutable, updated by running instances
```

**Key Insight**: 
- `Cell.json` = versioned schema, stored **in git**
- `instances.json` = live instance registry, stored **outside git**, managed by cell-git service

### 1.2 Cell-Git Service Definition

**File: `cells/cell-git/src/main.rs`**

```rust
use anyhow::Result;
use cell_sdk as cell;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize)]
struct InstanceRegistry {
    cell_name: String,
    version: String,
    instances: Vec<InstanceInfo>,
    updated_at: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct InstanceInfo {
    node_id: String,
    endpoint: String,
    region: Option<String>,
    last_heartbeat: String,
    signature: String,
}

#[cell::service]
#[derive(Clone)]
struct CellGitService {
    storage_root: PathBuf,
    // In-memory cache of instance registries (expires after 60s)
    instance_cache: Arc<RwLock<HashMap<String, (InstanceRegistry, std::time::Instant)>>>,
}

#[cell::handler]
impl CellGitService {
    /// Fetch a file from a repository at a specific ref
    async fn fetch_file(&self, repo: String, ref_name: String, path: String) -> Result<Vec<u8>> {
        let repo_path = self.storage_root.join("repos").join(&repo);
        
        // Use git2 to read from bare repo
        let repo = git2::Repository::open(&repo_path)?;
        let obj = repo.revparse_single(&ref_name)?;
        let commit = obj.peel_to_commit()?;
        let tree = commit.tree()?;
        let entry = tree.get_path(std::path::Path::new(&path))?;
        let blob = repo.find_blob(entry.id())?;
        
        Ok(blob.content().to_vec())
    }
    
    /// Get the manifest (Cell.json) for a cell at a specific version
    async fn get_manifest(&self, repo: String, tag: String) -> Result<Vec<u8>> {
        self.fetch_file(repo, tag, "Cell.json".to_string()).await
    }
    
    /// Get live instances for a cell (from mutable store, not git)
    async fn get_instances(&self, repo: String) -> Result<Vec<u8>> {
        let cache_key = repo.clone();
        
        // Check cache first (60s TTL)
        {
            let cache = self.instance_cache.read().await;
            if let Some((registry, timestamp)) = cache.get(&cache_key) {
                if timestamp.elapsed().as_secs() < 60 {
                    return Ok(serde_json::to_vec(registry)?);
                }
            }
        }
        
        // Cache miss or expired - read from disk
        let instances_path = self.storage_root
            .join("instances")
            .join(&repo)
            .join("instances.json");
        
        if !instances_path.exists() {
            return Ok(serde_json::to_vec(&InstanceRegistry {
                cell_name: repo.clone(),
                version: "unknown".into(),
                instances: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })?);
        }
        
        let data = tokio::fs::read(&instances_path).await?;
        let registry: InstanceRegistry = serde_json::from_slice(&data)?;
        
        // Update cache
        {
            let mut cache = self.instance_cache.write().await;
            cache.insert(cache_key, (registry.clone(), std::time::Instant::now()));
        }
        
        Ok(serde_json::to_vec(&registry)?)
    }
    
    /// Announce a running instance (heartbeat)
    async fn announce_instance(
        &self,
        repo: String,
        instance: InstanceInfo,
    ) -> Result<()> {
        let instances_path = self.storage_root
            .join("instances")
            .join(&repo)
            .join("instances.json");
        
        tokio::fs::create_dir_all(instances_path.parent().unwrap()).await?;
        
        // Read existing registry
        let mut registry = if instances_path.exists() {
            let data = tokio::fs::read(&instances_path).await?;
            serde_json::from_slice::<InstanceRegistry>(&data)?
        } else {
            InstanceRegistry {
                cell_name: repo.clone(),
                version: "unknown".into(),
                instances: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            }
        };
        
        // Remove stale instances (no heartbeat in 30 seconds)
        let now = chrono::Utc::now();
        registry.instances.retain(|i| {
            if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&i.last_heartbeat) {
                (now - last).num_seconds() < 30
            } else {
                false
            }
        });
        
        // Update or insert this instance
        if let Some(existing) = registry.instances.iter_mut().find(|i| i.node_id == instance.node_id) {
            *existing = instance;
        } else {
            registry.instances.push(instance);
        }
        
        registry.updated_at = now.to_rfc3339();
        
        // Write back atomically
        let tmp_path = instances_path.with_extension("tmp");
        tokio::fs::write(&tmp_path, serde_json::to_vec_pretty(&registry)?).await?;
        tokio::fs::rename(&tmp_path, &instances_path).await?;
        
        // Invalidate cache
        {
            let mut cache = self.instance_cache.write().await;
            cache.remove(&repo);
        }
        
        Ok(())
    }
    
    /// Clone/push support (standard git operations)
    async fn git_receive_pack(&self, repo: String, data: Vec<u8>) -> Result<Vec<u8>> {
        // Handle git push (standard git protocol)
        todo!("Implement git-receive-pack")
    }
    
    async fn git_upload_pack(&self, repo: String, data: Vec<u8>) -> Result<Vec<u8>> {
        // Handle git fetch/clone (standard git protocol)
        todo!("Implement git-upload-pack")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let storage_root = dirs::home_dir()
        .expect("No home dir")
        .join(".cell/git");
    
    tokio::fs::create_dir_all(&storage_root).await?;
    
    let service = CellGitService {
        storage_root,
        instance_cache: Arc::new(RwLock::new(HashMap::new())),
    };
    
    println!("[CellGit] Storage: {:?}", service.storage_root);
    service.serve("cell-git").await
}
```

### 1.3 Bootstrap Cell-Git

**The Bootstrap Problem**: How do we host cell-git if cell-git is a cell?

**Solution**: Ship **one** hard-coded cell-git instance with the SDK.

**File: `cell-sdk/src/bootstrap.rs`**

```rust
/// The canonical cell-git instance (hard-coded)
pub const CELL_GIT_BOOTSTRAP: &str = "cell.network:443";

/// Alternative community instances (discovered via DHT later)
pub const CELL_GIT_FALLBACKS: &[&str] = &[
    "git.cell.community:443",
    "localhost:9000", // For local dev
];

pub async fn resolve_cell_git() -> Result<Synapse> {
    // Try bootstrap first
    if let Ok(conn) = Synapse::grow_endpoint(CELL_GIT_BOOTSTRAP).await {
        return Ok(conn);
    }
    
    // Try fallbacks
    for fallback in CELL_GIT_FALLBACKS {
        if let Ok(conn) = Synapse::grow_endpoint(fallback).await {
            return Ok(conn);
        }
    }
    
    bail!("Could not reach any cell-git instance")
}
```

---

## Phase 2: Build Script - Cell Resolution

### 2.1 Build Script Logic

**File: `cell-sdk/templates/build.rs`**

```rust
use anyhow::{Result, bail};
use std::time::{Duration, Instant};
use cell_sdk::registry::*;

fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=Cell.toml");
    println!("cargo:rerun-if-changed=Cell.lock");
    
    // 1. Parse Cell.toml
    let manifest = CellManifest::load("Cell.toml")?;
    
    // 2. For each cell dependency
    for (name, spec) in manifest.cells {
        println!("cargo:warning=[Cell] Resolving '{}'...", name);
        
        // 3. Check localhost FIRST (instant)
        if check_local_socket(&name) {
            println!("cargo:warning=[Cell] '{}' found locally", name);
            generate_instances_file(&name, vec![format!("unix://{}.sock", name)])?;
            continue;
        }
        
        // 4. Fetch manifest from cell-git
        let remote_manifest = fetch_manifest_from_git(&spec)?;
        
        // Verify fingerprint matches
        let expected_fp = remote_manifest.fingerprint;
        
        // 5. Fetch live instances from cell-git
        let instances = fetch_instances_from_git(&spec)?;
        
        if instances.is_empty() {
            handle_no_instances(&name, &spec)?;
            continue;
        }
        
        // 6. Ping all instances (parallel, 500ms timeout)
        let start = Instant::now();
        let live = ping_all_parallel(&instances, Duration::from_millis(500))?;
        println!("cargo:warning=[Cell] Pinged {} instances in {:?}", 
                 instances.len(), start.elapsed());
        
        if live.is_empty() {
            handle_no_instances(&name, &spec)?;
            continue;
        }
        
        // 7. Apply policy
        let policy = spec.policy.unwrap_or_default();
        let best = live.iter().min_by_key(|i| i.latency_ms as u64).unwrap();
        
        println!("cargo:warning=[Cell] Best instance: {} ({}ms)", 
                 best.endpoint, best.latency_ms);
        
        if best.latency_ms > policy.max_latency_ms as f64 {
            if policy.auto_spawn {
                println!("cargo:warning=[Cell] Latency {}ms > {}ms, spawning locally", 
                         best.latency_ms, policy.max_latency_ms);
                spawn_local(&name, &spec)?;
                generate_instances_file(&name, vec![format!("unix://{}.sock", name)])?;
            } else {
                bail!("Cell '{}' latency {}ms exceeds max {}ms and auto_spawn=false", 
                      name, best.latency_ms, policy.max_latency_ms);
            }
        } else {
            // Use remote instances
            let endpoints: Vec<String> = live.iter()
                .map(|i| i.endpoint.clone())
                .collect();
            generate_instances_file(&name, endpoints)?;
        }
        
        // 8. Update Cell.lock
        update_cell_lock(&name, &live, expected_fp)?;
    }
    
    Ok(())
}

fn fetch_manifest_from_git(spec: &CellSpec) -> Result<CellManifest> {
    // Connect to cell-git
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut git = cell_sdk::bootstrap::resolve_cell_git().await?;
        
        // Fetch Cell.json from the repo
        let repo_path = extract_repo_path(&spec.git)?;
        let tag = spec.tag.as_deref().unwrap_or("main");
        
        let data = git.get_manifest(repo_path, tag.to_string()).await?;
        let manifest: CellManifest = serde_json::from_slice(&data)?;
        
        Ok(manifest)
    })
}

fn fetch_instances_from_git(spec: &CellSpec) -> Result<Vec<InstanceInfo>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut git = cell_sdk::bootstrap::resolve_cell_git().await?;
        
        let repo_path = extract_repo_path(&spec.git)?;
        
        let data = git.get_instances(repo_path).await?;
        let registry: InstanceRegistry = serde_json::from_slice(&data)?;
        
        Ok(registry.instances)
    })
}

fn handle_no_instances(name: &str, spec: &CellSpec) -> Result<()> {
    let policy = spec.policy.as_ref().unwrap_or(&Default::default());
    
    if policy.auto_spawn {
        println!("cargo:warning=[Cell] No instances found for '{}', spawning locally", name);
        spawn_local(name, spec)?;
        generate_instances_file(name, vec![format!("unix://{}.sock", name)])?;
        Ok(())
    } else {
        bail!("Cell '{}' has no running instances and auto_spawn=false", name)
    }
}

fn spawn_local(name: &str, spec: &CellSpec) -> Result<()> {
    use std::process::Command;
    
    println!("cargo:warning=[Cell] Synthesizing '{}'...", name);
    
    // 1. Clone repo (cached in ~/.cell/repos)
    let repo_dir = clone_repo(&spec.git, spec.tag.as_deref())?;
    
    // 2. Synthesize binary
    let binary = cell_sdk::ribosome::Ribosome::synthesize(&repo_dir, name)?;
    
    // 3. Spawn in background
    let socket_dir = cell_sdk::membrane::resolve_socket_dir();
    let umbilical = socket_dir.join("mitosis.sock");
    
    Command::new(&binary)
        .arg("--membrane")
        .env("CELL_SOCKET_DIR", &socket_dir)
        .spawn()?;
    
    // 4. Wait for socket (5s timeout)
    let socket_path = socket_dir.join(format!("{}.sock", name));
    let start = Instant::now();
    while !socket_path.exists() {
        if start.elapsed() > Duration::from_secs(5) {
            bail!("Timeout waiting for '{}' socket", name);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    
    println!("cargo:warning=[Cell] '{}' spawned successfully", name);
    Ok(())
}

fn clone_repo(git_url: &str, tag: Option<&str>) -> Result<PathBuf> {
    use git2::Repository;
    
    let cache_dir = dirs::home_dir().unwrap().join(".cell/repos");
    std::fs::create_dir_all(&cache_dir)?;
    
    // Hash the URL to create a unique cache key
    let mut hasher = blake3::Hasher::new();
    hasher.update(git_url.as_bytes());
    let hash = hasher.finalize().to_hex();
    
    let repo_dir = cache_dir.join(&hash[..16]);
    
    if !repo_dir.exists() {
        println!("cargo:warning=[Cell] Cloning {}...", git_url);
        Repository::clone(git_url, &repo_dir)?;
    }
    
    // Checkout specific tag/branch
    let repo = Repository::open(&repo_dir)?;
    if let Some(tag) = tag {
        let (obj, reference) = repo.revparse_ext(tag)?;
        repo.checkout_tree(&obj, None)?;
        match reference {
            Some(r) => repo.set_head(r.name().unwrap())?,
            None => repo.set_head_detached(obj.id())?,
        }
    }
    
    Ok(repo_dir)
}

fn ping_all_parallel(instances: &[InstanceInfo], timeout: Duration) -> Result<Vec<InstanceWithLatency>> {
    use rayon::prelude::*;
    
    let results: Vec<_> = instances.par_iter()
        .filter_map(|instance| {
            match ping_instance(&instance.endpoint, timeout) {
                Ok(latency_ms) => Some(InstanceWithLatency {
                    endpoint: instance.endpoint.clone(),
                    latency_ms,
                    node_id: instance.node_id.clone(),
                }),
                Err(e) => {
                    eprintln!("cargo:warning=[Cell] Ping failed {}: {}", instance.endpoint, e);
                    None
                }
            }
        })
        .collect();
    
    Ok(results)
}

fn ping_instance(endpoint: &str, timeout: Duration) -> Result<f64> {
    use std::net::TcpStream;
    
    let start = Instant::now();
    
    // Try TCP handshake
    let stream = TcpStream::connect_timeout(
        &endpoint.parse()?,
        timeout,
    )?;
    drop(stream);
    
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

fn check_local_socket(name: &str) -> bool {
    let socket_dir = cell_sdk::membrane::resolve_socket_dir();
    let socket_path = socket_dir.join(format!("{}.sock", name));
    socket_path.exists()
}

fn generate_instances_file(name: &str, endpoints: Vec<String>) -> Result<()> {
    let out_dir = std::env::var("OUT_DIR")?;
    let path = format!("{}/{}_instances.rs", out_dir, name);
    
    let code = format!(
        "&[{}]",
        endpoints.iter()
            .map(|e| format!("\"{}\"", e))
            .collect::<Vec<_>>()
            .join(", ")
    );
    
    std::fs::write(path, code)?;
    Ok(())
}

fn update_cell_lock(name: &str, instances: &[InstanceWithLatency], fingerprint: u64) -> Result<()> {
    // TODO: Implement Cell.lock writing
    Ok(())
}

fn extract_repo_path(git_url: &str) -> Result<String> {
    // Extract "org/repo" from URLs like:
    // https://cell.network/org/repo
    // https://github.com/org/repo
    
    let parts: Vec<&str> = git_url.trim_end_matches('/').split('/').collect();
    if parts.len() < 2 {
        bail!("Invalid git URL: {}", git_url);
    }
    
    Ok(format!("{}/{}", parts[parts.len()-2], parts[parts.len()-1]))
}

struct InstanceWithLatency {
    endpoint: String,
    latency_ms: f64,
    node_id: String,
}
```

---

## Phase 3: Instance Heartbeat

Cells need to announce themselves to cell-git.

### 3.1 Heartbeat Service

**File: `cell-sdk/src/heartbeat.rs`**

```rust
use anyhow::Result;
use std::sync::Arc;
use tokio::time::{interval, Duration};

pub struct HeartbeatService {
    cell_name: String,
    node_id: String,
    endpoint: String,
}

impl HeartbeatService {
    pub fn new(cell_name: String, endpoint: String) -> Self {
        let node_id = generate_node_id(&cell_name);
        Self {
            cell_name,
            node_id,
            endpoint,
        }
    }
    
    pub async fn start(self: Arc<Self>) {
        let mut ticker = interval(Duration::from_secs(10));
        
        loop {
            ticker.tick().await;
            
            if let Err(e) = self.send_heartbeat().await {
                eprintln!("[Heartbeat] Failed: {}", e);
            }
        }
    }
    
    async fn send_heartbeat(&self) -> Result<()> {
        // Connect to cell-git
        let mut git = crate::bootstrap::resolve_cell_git().await?;
        
        // Announce ourselves
        let instance = InstanceInfo {
            node_id: self.node_id.clone(),
            endpoint: self.endpoint.clone(),
            region: None,
            last_heartbeat: chrono::Utc::now().to_rfc3339(),
            signature: sign_instance(&self.node_id, &self.endpoint),
        };
        
        git.announce_instance(self.cell_name.clone(), instance).await?;
        
        Ok(())
    }
}

fn generate_node_id(cell_name: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(cell_name.as_bytes());
    hasher.update(&rand::random::<u64>().to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn sign_instance(node_id: &str, endpoint: &str) -> String {
    // TODO: Implement ed25519 signature
    format!("{}:{}", node_id, endpoint)
}
```

### 3.2 Auto-Start Heartbeat in Membrane

**File: `cell-sdk/src/membrane.rs` (modify)**

```rust
impl Membrane {
    pub async fn bind<F, Req, Resp>(
        name: &str,
        handler: F,
        genome_json: Option<String>,
    ) -> Result<()>
    where
        // ... (existing bounds)
    {
        // ... (existing socket binding code)
        
        // NEW: Start heartbeat service
        let endpoint = detect_public_endpoint()?;
        let heartbeat = Arc::new(crate::heartbeat::HeartbeatService::new(
            name.to_string(),
            endpoint,
        ));
        
        tokio::spawn(async move {
            heartbeat.start().await;
        });
        
        // ... (existing accept loop)
    }
}

fn detect_public_endpoint() -> Result<String> {
    // Try to detect if we're publicly reachable
    // For now, just use local IP + random port
    
    use local_ip_address::local_ip;
    let ip = local_ip()?;
    
    // TODO: Detect actual bound port
    Ok(format!("{}:8080", ip))
}
```

---

## Phase 4: Cell CLI

### 4.1 Main Commands

**File: `cell-cli/Cargo.toml`**

```toml
[package]
name = "cell-cli"
version = "0.3.0"
edition = "2021"

[[bin]]
name = "cell"
path = "src/main.rs"

[dependencies]
cell-sdk = { path = "../cell-sdk" }
clap = { version = "4.5", features = ["derive"] }
anyhow = "1.0"
tokio = { version = "1", features = ["full"] }
toml = "0.8"
serde = { version = "1.0", features = ["derive"] }
```

**File: `cell-cli/src/main.rs`**

```rust
use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Parser)]
#[command(name = "cell")]
#[command(about = "Cell - Biological Distributed Computing", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new Cell project
    Init { name: String },
    
    /// Add a cell dependency
    Add {
        name: String,
        #[arg(long)]
        git: String,
        #[arg(long)]
        tag: Option<String>,
    },
    
    /// Run a cell locally
    Run { name: String },
    
    /// List running cells
    Ps,
    
    /// Start cell-git server
    Git,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Init { name } => cmd_init(&name),
        Commands::Add { name, git, tag } => cmd_add(&name, &git, tag.as_deref()),
        Commands::Run { name } => cmd_run(&name).await,
        Commands::Ps => cmd_ps(),
        Commands::Git => cmd_git().await,
    }
}

fn cmd_init(name: &str) -> Result<()> {
    println!("🧬 Initializing cell project '{}'", name);
    
    std::fs::create_dir_all(format!("{}/src", name))?;
    
    // Cell.toml
    let cell_toml = format!(r#"[package]
name = "{}"
version = "0.1.0"

[cell]
# Remote cell dependencies
# example = {{ git = "https://cell.network/org/example", tag = "v1.0.0" }}

[registry]
hub = "cell.network:443"
"#, name);
    std::fs::write(format!("{}/Cell.toml", name), cell_toml)?;
    
    // Cargo.toml
    let cargo_toml = format!(r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
cell-sdk = "0.3"
anyhow = "1.0"
tokio = {{ version = "1", features = ["full"] }}

[build-dependencies]
cell-sdk = {{ version = "0.3", features = ["build"] }}
"#, name);
    std::fs::write(format!("{}/Cargo.toml", name), cargo_toml)?;
    
    // build.rs
    let build_rs = include_str!("../../cell-sdk/templates/build.rs");
    std::fs::write(format!("{}/build.rs", name), build_rs)?;
    
    // src/main.rs
    let main_rs = r#"use anyhow::Result;
use cell_sdk as cell;

#[cell::service]
#[derive(Clone)]
struct MyService;

#[cell::handler]
impl MyService {
    async fn hello(&self, name: String) -> Result<String> {
        Ok(format!("Hello, {}!", name))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let service = MyService;
    service.serve("my-service").await
}
"#;
    std::fs::write(format!("{}/src/main.rs", name), main_rs)?;
    
    println!("✓ Created {}/", name);
    println!("  cd {}", name);
    println!("  cargo build");
    
    Ok(())
}

fn cmd_add(name: &str, git: &str, tag: Option<&str>) -> Result<()> {
    println!("📦 Adding dependency: {}", name);
    
    let mut config: toml::Value = toml::from_str(&std::fs::read_to_string("Cell.toml")?)?;
    
    let cell_table = config.get_mut("cell")
        .and_then(|v| v.as_table_mut())
        .ok_or_else(|| anyhow::anyhow!("No [cell] section"))?;
    
    let mut dep = toml::map::Map::new();
    dep.insert("git".into(), toml::Value::String(git.into()));
    if let Some(t) = tag {
        dep.insert("tag".into(), toml::Value::String(t.into()));
    }
    
    cell_table.insert(name.into(), toml::Value::Table(dep));
    
    std::fs::write("Cell.toml", toml::to_string(&config)?)?;
    
    println!("✓ Added {} = {{ git = \"{}\", tag = \"{}\" }}", 
             name, git, tag.unwrap_or("main"));
    println!("  Run `cargo build` to resolve");
    
    Ok(())
}

async fn cmd_run(name: &str) -> Result<()> {
    println!("🚀 Running cell '{}'", name);
    
    // Just cargo run in the current directory
    std::process::Command::new("cargo")
        .arg("run")
        .arg("--release")
        .status()?;
    
    Ok(())
}

fn cmd_ps() -> Result<()> {
    println!("📡 Running cells:");
    
    let socket_dir = cell_sdk::membrane::resolve_socket_dir();
    
    for entry in std::fs::read_dir(socket_dir)? {
        let entry = entry?;
        let path = entry.path();
        
        if path.extension().and_then(|s| s.to_str()) == Some("sock") {
            let name = path.file_stem().unwrap().to_str().unwrap();
            println!("  • {}", name);
        }
    }
    
    Ok(())
}

async fn cmd_git() -> Result<()> {
    println!("🌐 Starting cell-git server...");
    
    // Spawn cell-git binary
    std::process::Command::new("cell-git")
        .spawn()?
        .wait()?;
    
    Ok(())
}
```

---

## Implementation TODO:

### 1: Cell-Git Foundation
- [ ] Implement `CellGitService` with basic file fetching
- [ ] Add instance registry (mutable store)
- [ ] Add heartbeat announcement endpoint
- [ ] Test locally with mock data

### 2: Build Script
- [ ] Implement `build.rs` template
- [ ] Add cell-git client in build context
- [ ] Add parallel pinging
- [ ] Add auto-spawn logic
- [ ] Test with local cell-git

### 3: Heartbeat & Integration
- [ ] Add heartbeat service to Membrane
- [ ] Add instance signature verification
- [ ] Add 60s cache expiry
- [ ] End-to-end test: spawn cell → heartbeat → discover

### 4: CLI & Polish
- [ ] Implement `cell` CLI tool
- [ ] Add `cell init`, `cell add`, `cell ps`
- [ ] Add `cell git` launcher
- [ ] Documentation

### 5: Production Hardening
- [ ] Add git-receive-pack / git-upload-pack
- [ ] Add proper ed25519 signatures
- [ ] Add rate limiting to cell-git
- [ ] Deploy first public cell.network instance

---

## First Milestone: Local Test

Let's prove this works **locally** before touching network:

```bash
# Terminal 1: Start cell-git
cargo run --bin cell-git

# Terminal 2: Create and run exchange cell
cell init exchange
cd exchange
# (modify src/main.rs with exchange logic)
cargo build --release
./target/release/exchange

# Terminal 3: Create trader that depends on exchange
cell init trader
cd trader
cell add exchange --git http://localhost:9000/local/exchange --tag main
# (modify src/main.rs to use ExchangeClient)
cargo build  # Should ping local exchange, find it, succeed
cargo run
```

If this works, we have:
- Cell-git serving manifests
- Build script fetching + pinging
- Heartbeat keeping instance list fresh
- Auto-spawn when needed

Then we deploy to `cell.network` and suddenly **global**.
