use anyhow::Result;
use cell_sdk::{MyceliumRoot, Synapse, protein};
use std::path::Path;
use tokio::fs;

// --- PROTOCOL (Shared DNA) ---
const PROTOCOL_SRC: &str = r#"
use cell_sdk::protein;

#[protein]
pub enum MarketMsg {
    PlaceOrder { symbol: String, amount: u64, side: u8 }, // 0=Buy, 1=Sell
    OrderAck { id: u64 },
    SnapshotRequest,
    SnapshotResponse { total_trades: u64 },
}
"#;

// --- MAIN ORCHESTRATOR ---

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Setup the DNA Library
    setup_dna().await?;

    println!("\n=== SYSTEM IGNITION ===");
    
    // 2. Start the Root
    let _root = MyceliumRoot::ignite().await?;
    println!("[Genesis] Mycelium Root Active.");

    // 3. Spawn the Exchange
    println!("[Genesis] Growing Exchange Cell...");
    let mut exchange_conn = Synapse::grow("exchange").await?;
    println!("[Genesis] Exchange Online.");

    // 4. Verification Loop
    let start = std::time::Instant::now();
    
    println!("[Genesis] Monitoring Market Stability...");
    for i in 0..5 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        
        let req = MarketMsg::SnapshotRequest;
        let resp_vesicle = exchange_conn.fire(req).await?;
        
        let resp = cell_sdk::rkyv::from_bytes::<MarketMsg>(resp_vesicle.as_slice())
            .map_err(|e| anyhow::anyhow!("Deserialization failed: {:?}", e))?;
        
        if let MarketMsg::SnapshotResponse { total_trades } = resp {
            let rps = total_trades as f64 / start.elapsed().as_secs_f64();
            println!("[Metric] T+{}: Total Trades: {} | {:.0} TPS", i, total_trades, rps);
        }
    }

    println!("\n=== SIMULATION COMPLETE ===");
    Ok(())
}

// --- HELPER: WRITE SOURCE CODE TO DISK ---
async fn setup_dna() -> Result<()> {
    let home = dirs::home_dir().unwrap();
    let dna_root = home.join(".cell/dna");

    // --- CLEANUP CACHE TO FORCE RECOMPILE ---
    let cache_dir = home.join(".cell/cache/release");
    if cache_dir.exists() {
        let _ = fs::remove_file(cache_dir.join("exchange")).await;
        let _ = fs::remove_file(cache_dir.join("trader")).await;
    }

    // --- 1. THE EXCHANGE CELL SOURCE ---
    let exchange_dir = dna_root.join("exchange");
    write_project(
        &exchange_dir, 
        "exchange", 
        r#"
use anyhow::Result;
use cell_sdk::{Membrane, Synapse}; 
use cell_consensus::{RaftNode, RaftConfig, StateMachine};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

include!("protocol.rs"); 

struct MarketState {
    trade_count: AtomicU64,
}
impl StateMachine for MarketState {
    fn apply(&self, _command: &[u8]) {
        self.trade_count.fetch_add(1, Ordering::Relaxed);
    }
    fn snapshot(&self) -> Vec<u8> { vec![] }
    fn restore(&self, _snapshot: &[u8]) {}
}

#[tokio::main]
async fn main() -> Result<()> {
    let state = Arc::new(MarketState { trade_count: AtomicU64::new(0) });
    
    // WAL at /tmp to work in read-only container
    let wal_path = std::path::PathBuf::from("/tmp/market.wal");

    let config = RaftConfig {
        id: 1,
        storage_path: wal_path,
    };
    let raft = RaftNode::new(config, state.clone()).await?;

    println!("[Exchange] Consensus Active. Spawning Traders...");

    for i in 0..5 {
        tokio::spawn(async move {
            // We spawn the trader but ignore the result.
            // The trader is 'headless' (doesn't bind a socket), so Synapse::grow 
            // will timeout waiting for a connection. We ignore this specific error 
            // because we know they are running as fire-and-forget workers.
            let _ = Synapse::grow("trader").await;
        });
    }

    println!("[Exchange] Listening for orders...");

    Membrane::bind("exchange", move |vesicle| {
        let raft = raft.clone();
        let state = state.clone();
        
        async move {
            let msg = cell_sdk::rkyv::from_bytes::<MarketMsg>(vesicle.as_slice())
                .map_err(|e| anyhow::anyhow!("Msg Error: {:?}", e))?;
            
            match msg {
                MarketMsg::PlaceOrder { symbol: _, amount: _, side: _ } => {
                    let _ = raft.propose(vec![1]).await.unwrap();
                    
                    let ack = MarketMsg::OrderAck { id: 1 };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&ack)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                MarketMsg::SnapshotRequest => {
                    let count = state.trade_count.load(Ordering::Relaxed);
                    let resp = MarketMsg::SnapshotResponse { total_trades: count };
                    let bytes = cell_sdk::rkyv::to_bytes::<_, 16>(&resp)?.into_vec();
                    Ok(cell_sdk::vesicle::Vesicle::wrap(bytes))
                }
                _ => Ok(vesicle)
            }
        }
    }).await
}
"#).await?;

    // --- 2. THE TRADER CELL SOURCE ---
    let trader_dir = dna_root.join("trader");
    write_project(
        &trader_dir, 
        "trader", 
        r#"
use anyhow::Result;
use cell_sdk::{Synapse}; 
use std::time::Duration;

include!("protocol.rs");

#[tokio::main]
async fn main() -> Result<()> {
    // Retry connection loop
    let mut conn = loop {
        match Synapse::grow("exchange").await {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };
    
    // Rate limited loop
    loop {
        let order = MarketMsg::PlaceOrder { 
            symbol: "CELL".to_string(), 
            amount: 100, 
            side: 0 
        };
        
        // FIXED: Passed by value (ownership transfer) to satisfy rkyv::Serialize
        if let Err(e) = conn.fire(order).await {
            eprintln!("Trader error: {}", e);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        
        tokio::task::yield_now().await; 
    }
}
"#).await?;

    Ok(())
}

async fn write_project(dir: &Path, name: &str, code: &str) -> Result<()> {
    fs::create_dir_all(dir.join("src")).await?;
    fs::write(dir.join("src/protocol.rs"), PROTOCOL_SRC).await?;
    fs::write(dir.join("src/main.rs"), code).await?;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let root_path = Path::new(&manifest_dir).parent().unwrap().parent().unwrap();
    let sdk_path = root_path.join("cell-sdk").display().to_string();
    let consensus_path = root_path.join("cell-consensus").display().to_string();

    let toml = format!(r#"
[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1.0"
tokio = {{ version = "1", features = ["full"] }}
cell-sdk = {{ path = "{}" }}
cell-consensus = {{ path = "{}" }}
"#, name, sdk_path, consensus_path);

    fs::write(dir.join("Cargo.toml"), toml).await?;
    Ok(())
}

#[protein]
pub enum MarketMsg {
    PlaceOrder { symbol: String, amount: u64, side: u8 },
    OrderAck { id: u64 },
    SnapshotRequest,
    SnapshotResponse { total_trades: u64 },
}