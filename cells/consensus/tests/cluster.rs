use cell_test_support::*;
use cell_sdk::*;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;
use std::collections::HashMap;

// Define the RPC interface for the test client
cell_remote!(Raft = "consensus-raft");

async fn install_consensus_dna() -> Result<()> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let source_path = PathBuf::from(manifest_dir).parent().unwrap().join("consensus");
    
    let root_socket_dir = std::env::var("CELL_SOCKET_DIR").unwrap();
    let home = PathBuf::from(root_socket_dir).join("home");
    
    // We install it as "consensus" (the generic DNA)
    let dna_dir = home.join(".cell/dna").join("consensus");
    
    if dna_dir.exists() {
        std::fs::remove_dir_all(&dna_dir)?;
    }
    std::fs::create_dir_all(&dna_dir)?;
    
    let options = fs_extra::dir::CopyOptions::new().content_only(true);
    fs_extra::dir::copy(&source_path, &dna_dir, &options)?;
    
    Ok(())
}

fn identity_config(id: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("CELL_IDENTITY".to_string(), id.to_string());
    map
}

async fn wait_for_leader(nodes: &[&str]) -> Result<(String, Raft::Client)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        for name in nodes {
            // Note: In this setup, the socket is named after the Identity (e.g., "Alpha")
            // because service.serve() uses the ID.
            if let Ok(mut client) = Raft::Client::new(spawn(name).await) {
                let cmd = Raft::Command { data: b"ping".to_vec() };
                if client.propose(cmd).await.is_ok() {
                    return Ok((name.to_string(), client));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("Timed out waiting for leader election");
}

#[tokio::test]
async fn static_topology_cluster() -> Result<()> {
    // 1. Ignite Substrate
    let _root = root().await;

    // 2. Install the Stem Cell DNA
    install_consensus_dna().await?;

    // 3. Differentiate Nodes
    info!("Differentiating Stem Cells into Alpha, Beta, Gamma...");
    
    // WORKAROUND FOR TEST: We copy the DNA to "Alpha", "Beta", "Gamma" directories 
    // so Root finds them based on the spawn request name, but they are identical sources.
    
    install_cell_source_as("Alpha").await?;
    install_cell_source_as("Beta").await?;
    install_cell_source_as("Gamma").await?;

    async fn install_cell_source_as(name: &str) -> Result<()> {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
        let source_path = PathBuf::from(manifest_dir).parent().unwrap().join("consensus");
        let root_socket_dir = std::env::var("CELL_SOCKET_DIR").unwrap();
        let home = PathBuf::from(root_socket_dir).join("home");
        let dna_dir = home.join(".cell/dna").join(name);
        if dna_dir.exists() { std::fs::remove_dir_all(&dna_dir)?; }
        std::fs::create_dir_all(&dna_dir)?;
        let options = fs_extra::dir::CopyOptions::new().content_only(true);
        fs_extra::dir::copy(&source_path, &dna_dir, &options)?;
        Ok(())
    }

    let _s1 = spawn_with_config("Alpha", identity_config("Alpha")).await;
    let _s2 = spawn_with_config("Beta", identity_config("Beta")).await;
    let _s3 = spawn_with_config("Gamma", identity_config("Gamma")).await;

    info!("Cluster differentiated. Waiting for election...");
    tokio::time::sleep(Duration::from_secs(5)).await;

    let nodes = vec!["Alpha", "Beta", "Gamma"];
    let (leader, mut client) = wait_for_leader(&nodes).await?;
    info!("Leader is {:?}", leader);

    // Verify Graph Integrity
    let res = client.propose(Raft::Command { data: b"static_topology_works".to_vec() }).await?;
    assert!(res.index > 0);
    
    info!("Static Topology Consensus Verified.");
    Ok(())
}