use cell_sdk::*;
use cell_model::config::{CellInitConfig, PeerConfig};
use anyhow::Result;
use std::time::Duration;
use tracing::info;

// Define the RPC interface for the test client
cell_remote!(Raft = "consensus");

// Helper to wait for a leader to be elected among the cluster
async fn wait_for_leader(nodes: &[&str]) -> Result<(String, Raft::Client)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        for name in nodes {
            // We try to connect to the specific socket name (e.g. "Alpha")
            // Synapse resolves this in the current organism scope automatically
            if let Ok(conn) = Synapse::grow(name).await {
                let mut client = Raft::Client::new(conn);
                let cmd = Raft::Command { data: b"ping".to_vec() };
                
                // If propose succeeds, this node is the leader
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
    // 1. Boot the environment
    cell_sdk::System::ignite_local_cluster().await?;

    info!("Spawning Cluster: Alpha, Beta, Gamma...");

    // 2. Define Topologies
    // We explicitly configure the socket paths so we can connect to them by name later.
    // The socket directory is resolved relative to the test environment by the SDK.
    let socket_dir = cell_sdk::resolve_socket_dir();
    
    let peers_config = vec![
        (1, "Alpha"),
        (2, "Beta"),
        (3, "Gamma"),
    ];

    for (id, name) in &peers_config {
        let peers: Vec<PeerConfig> = peers_config.iter()
            .filter(|(pid, _)| pid != id)
            .map(|(pid, pname)| PeerConfig {
                node_id: *pid,
                address: pname.to_string(), // In test env, address = cell name
            })
            .collect();

        let config = CellInitConfig {
            node_id: *id,
            cell_name: name.to_string(),
            peers,
            // We bind to {socket_dir}/{name}.sock
            socket_path: socket_dir.join(format!("{}.sock", name)).to_string_lossy().to_string(),
            organism: "system".to_string(), // Run in system scope for this test
        };

        // Spawn using the 'consensus' DNA, but inject the specific identity config
        System::spawn("consensus", Some(config)).await?;
    }

    // 3. Wait for all sockets to be ready
    Synapse::grow_await("Alpha").await?;
    Synapse::grow_await("Beta").await?;
    Synapse::grow_await("Gamma").await?;

    info!("Cluster active. Waiting for election...");

    // 4. Verification
    let nodes = vec!["Alpha", "Beta", "Gamma"];
    let (leader, mut client) = wait_for_leader(&nodes).await?;
    
    info!("Elected Leader: {}", leader);

    // Verify Graph Integrity via Log Replication
    let res = client.propose(Raft::Command { data: b"static_topology_works".to_vec() }).await?;
    assert!(res.index > 0);
    
    info!("Consensus Verified.");
    Ok(())
}