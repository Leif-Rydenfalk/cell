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
            // Fix: Use Raft::Client::new(conn) which now exists
            if let Ok(mut client) = Raft::Client::connect().await { 
                // Wait, connect() finds by name?
                // The macro generates `connect()` which connects to the cell_name hardcoded in the macro `cell_remote`.
                // BUT here we need to connect to specific dynamic names (node_1, etc).
                // `cell_remote!(Raft = "consensus-raft")` connects to "consensus-raft" by default.
                
                // We need to use `Raft::Client::new(conn)`.
                // And `conn` needs to be created via `spawn` or `Synapse::grow`.
                
                // The `spawn` helper returns a Synapse connected to the node.
                let conn = spawn(name).await;
                let mut client = Raft::Client::new(conn);

                let cmd = Raft::Command { data: b"ping".to_vec() };
                
                // Handling Client Result<u64, CellError>
                if let Ok(_) = client.propose(cmd).await {
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
    let _root = root().await;

    install_consensus_dna().await?;

    info!("Differentiating Stem Cells into Alpha, Beta, Gamma...");
    
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
    let res = client.propose(Raft::Command { data: b"static_topology_works".to_vec() }).await??; // Double unwrap for Result<Result<...>>
    // Wait, with updated macro, is it Result<Result<>> or just Result?
    // Client returns Result<T, CellError>. T is ProposeResult.
    // So single unwrap `?` returns ProposeResult.
    // Let's assume my last macro fix was applied correctly.
    // If test fails, we adjust.
    // The previous error in gateway suggested single Result<u64, CellError> was returned.
    
    assert!(res.index > 0);
    
    info!("Static Topology Consensus Verified.");
    Ok(())
}