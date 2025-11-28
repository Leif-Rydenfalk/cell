use anyhow::{Context, Result};
use cell_sdk::{MyceliumRoot, Synapse, protein};
use std::fs;
use std::path::Path;
use std::time::Duration;
use toml_edit::{DocumentMut, value};

// --- SCHEMA DEFINITION (Orchestrator) ---
// The orchestrator acts as a client here.
// It will check ~/.cell/schema/MarketV1.lock just like the Trader does.
#[protein(class = "MarketV1")]
pub enum MarketMsg {
    PlaceOrder {
        symbol: String,
        amount: u64,
        side: u8,
    },
    SubmitBatch {
        count: u32,
    },
    OrderAck {
        id: u64,
    },
    SnapshotRequest,
    SnapshotResponse {
        total_trades: u64,
    },
}
// ----------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let example_root = std::env::current_dir().context("Failed to get current dir")?;
    let cells_source = example_root.join("cells");
    
    let framework_root = example_root.parent().unwrap().parent().unwrap();
    let sdk_path = framework_root.join("cell-sdk").canonicalize()?;
    let consensus_path = framework_root.join("cell-consensus").canonicalize()?;

    // Smart Deploy
    deploy_and_patch(&cells_source, &sdk_path, &consensus_path).await?;

    println!("\n=== SYSTEM IGNITION ===");
    let _root = MyceliumRoot::ignite().await?;
    println!("[Genesis] Mycelium Root Active.");

    println!("[Genesis] Growing Exchange...");
    let mut exchange_conn = Synapse::grow("exchange").await?;
    println!("[Genesis] Exchange Online.");

    // Wait for traders
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("[Genesis] Monitoring Market Stability...");
    let start = std::time::Instant::now();

    for i in 0..10 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        
        // Use our local definition
        let req = MarketMsg::SnapshotRequest;
        
        if let Ok(resp_vesicle) = exchange_conn.fire(req).await {
            // Deserialize using our local definition
            if let Ok(MarketMsg::SnapshotResponse { total_trades }) = 
                cell_sdk::rkyv::from_bytes::<MarketMsg>(resp_vesicle.as_slice()) {
                
                let elapsed = start.elapsed().as_secs_f64();
                let rps = total_trades as f64 / elapsed;
                println!("[Metric] T+{}s: Total Trades: {} | {:.2} TPS", i, total_trades, rps);
            }
        }
    }
    println!("\n=== SIMULATION COMPLETE ===");
    Ok(())
}

async fn deploy_and_patch(src_cells: &Path, sdk_path: &Path, consensus_path: &Path) -> Result<()> {
    let home = dirs::home_dir().context("Home dir not found")?;
    let dna_root = home.join(".cell/dna");
    fs::create_dir_all(&dna_root)?;

    println!("[Deploy] Syncing DNA to {:?}", dna_root);
    
    // REMOVED: "protocol" from this list
    let components = vec!["exchange", "trader"];

    for component in components {
        let src = src_cells.join(component);
        let dst = dna_root.join(component);

        copy_dir_recursive(&src, &dst)?;

        let toml_path = dst.join("Cargo.toml");
        patch_cargo_toml(&toml_path, sdk_path, consensus_path)?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

fn patch_cargo_toml(path: &Path, sdk_path: &Path, consensus_path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)?;
    let mut doc = content.parse::<DocumentMut>()?;

    let replace_path = |doc: &mut DocumentMut, dep: &str, new_path: &Path| {
        if let Some(item) = doc.get_mut("dependencies").and_then(|d| d.get_mut(dep)) {
            if item.get("path").is_some() {
                item["path"] = value(new_path.to_string_lossy().to_string());
            }
        }
    };

    replace_path(&mut doc, "cell-sdk", sdk_path);
    replace_path(&mut doc, "cell-consensus", consensus_path);
    
    if doc.to_string() != content {
        fs::write(path, doc.to_string())?;
    }
    Ok(())
}