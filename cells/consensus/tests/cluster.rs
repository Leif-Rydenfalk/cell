use cell_test_support::*;
use cell_sdk::*;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;
use std::collections::HashMap;

// Define the RPC interface for the test client
// This uses the macro to find "cells/consensus/src/main.rs"
cell_remote!(Raft = "consensus");

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
            
            // spawn(name) waits for the socket "name.sock" to appear
            // and returns a Synapse connected to it.
            let conn = spawn(name).await;
            
            // Create a typed client using the existing connection
            let mut client = Raft::Client::new(conn);

            let cmd = Raft::Command { data: b"ping".to_vec() };
            
            // Try to propose. If it succeeds, we found the leader.
            // Result<ProposeResult, CellError>
            if let Ok(_) = client.propose(cmd).await {
                return Ok((name.to_string(), client));
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("Timed out waiting for leader election");
}

#[tokio::test]
async fn static_topology_cluster() -> Result<()> {
    let _root = root().await;

    install_consensus_dna().await?;

    info!("Differentiating Stem Cells into Alpha, Beta, Gamma...");
    
    // We spawn 3 nodes. The test helper `spawn_with_config` calls Root.
    // Root injects the config.
    // But `spawn` waits for "Alpha.sock".
    // Does Root create the socket? No, the Cell does.
    // The Cell reads identity "Alpha", binds "Alpha.sock".
    // `spawn` succeeds.
    
    // Note: We need to install the source as "Alpha", "Beta", "Gamma" because
    // Root looks for DNA matching the requested name.
    // Ideally Root supports `spawn(dna="consensus", name="Alpha")`.
    // But our current protocol is `Spawn { cell_name }` which implies DNA name.
    // So we copy DNA to Alpha/Beta/Gamma folders to satisfy the Root's lookup.
    
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