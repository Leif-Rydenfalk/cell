use anyhow::{Context, Result};
use cell_sdk::{MyceliumRoot, Synapse};
use protocol::MarketMsg;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
// FIX: Use correct crate and struct name (DocumentMut)
use toml_edit::{DocumentMut, value};

#[tokio::main]
async fn main() -> Result<()> {
    let example_root = std::env::current_dir().context("Failed to get current dir")?;
    let cells_source = example_root.join("cells");
    
    // Calculate absolute paths to the framework crates
    let framework_root = example_root
        .parent().unwrap() // examples/
        .parent().unwrap(); // cell-engine/
    
    let sdk_path = framework_root.join("cell-sdk").canonicalize()?;
    let consensus_path = framework_root.join("cell-consensus").canonicalize()?;

    if !cells_source.exists() {
        anyhow::bail!("Cells directory not found at {:?}. Run from 'examples/cell-market'", cells_source);
    }

    // Deploy and patch Cargo.toml
    deploy_and_patch(&cells_source, &sdk_path, &consensus_path).await?;

    println!("\n=== SYSTEM IGNITION ===");

    // Start the Root
    let _root = MyceliumRoot::ignite().await?;
    println!("[Genesis] Mycelium Root Active.");

    // Spawn Exchange
    println!("[Genesis] Growing Exchange Cell (this triggers compilation)...");
    let mut exchange_conn = Synapse::grow("exchange").await?;
    println!("[Genesis] Exchange Online.");

    // Verification Loop
    let start = std::time::Instant::now();
    println!("[Genesis] Monitoring Market Stability...");

    // Allow time for traders to spin up
    tokio::time::sleep(Duration::from_secs(2)).await;

    for i in 0..10 {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let req = MarketMsg::SnapshotRequest;
        match exchange_conn.fire(req).await {
            Ok(resp_vesicle) => {
                let resp = cell_sdk::rkyv::from_bytes::<MarketMsg>(resp_vesicle.as_slice())
                    .map_err(|e| anyhow::anyhow!("Deserialization failed: {:?}", e))?;

                if let MarketMsg::SnapshotResponse { total_trades } = resp {
                    let elapsed = start.elapsed().as_secs_f64();
                    let rps = total_trades as f64 / elapsed;
                    println!("[Metric] T+{}s: Total Trades: {} | {:.2} TPS", i, total_trades, rps);
                }
            }
            Err(e) => eprintln!("[Metric] Failed to query exchange: {}", e),
        }
    }

    println!("\n=== SIMULATION COMPLETE ===");
    Ok(())
}

async fn deploy_and_patch(
    src_cells: &Path, 
    sdk_path: &Path, 
    consensus_path: &Path
) -> Result<()> {
    let home = dirs::home_dir().context("Home dir not found")?;
    let dna_root = home.join(".cell/dna");

    println!("[Deploy] Installing DNA to {:?}", dna_root);

    let components = vec!["protocol", "exchange", "trader"];

    for component in components {
        let src = src_cells.join(component);
        let dst = dna_root.join(component);

        if dst.exists() {
            fs::remove_dir_all(&dst)?;
        }
        copy_dir_all(&src, &dst)?;

        let toml_path = dst.join("Cargo.toml");
        patch_cargo_toml(&toml_path, sdk_path, consensus_path)?;
    }

    Ok(())
}

fn patch_cargo_toml(path: &Path, sdk_path: &Path, consensus_path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)?;
    // FIX: Parse into DocumentMut
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
    
    fs::write(path, doc.to_string())?;
    Ok(())
}

fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}