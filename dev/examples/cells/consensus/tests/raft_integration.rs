#[cfg(test)]
mod tests {
    use cell_sdk::*;
    use anyhow::Result;

    // Define the remote interface for the consensus cell
    cell_remote!(Raft = "consensus");

    #[tokio::test]
    async fn raft_survives_follower_crash() {
        // Boot the test environment
        cell_sdk::System::ignite_local_cluster().await.unwrap();

        // Spawn a consensus node
        System::spawn("consensus", None).await.expect("Failed to spawn consensus");
        
        // Wait for it to be ready
        let mut synapse = Synapse::grow_await("consensus").await.expect("Failed to connect");
        
        // Construct the low-level request bytes manually as we are testing raw firing
        // (Or use the typed client if we preferred, but preserving the spirit of the original test)
        #[derive(cell_sdk::rkyv::Archive, cell_sdk::rkyv::Serialize, cell_sdk::rkyv::Deserialize)]
        #[archive(check_bytes)]
        #[archive(crate = "cell_sdk::rkyv")]
        struct Command {
            data: Vec<u8>
        }
        
        // The protocol expects Command inside a wrapper usually, but checking main.rs:
        // Service methods: propose(data: Vec<u8>) -> Result<u64>
        // So the wire format matches the generated protocol from `cell_remote!`.
        
        // Let's use the typed client to be safe and robust
        let mut client = Raft::Client::new(synapse);
        
        let cmd = Raft::Command { data: b"test_datum".to_vec() };
        let resp = client.propose(cmd).await;
        
        assert!(resp.is_ok(), "Leader should accept proposal");
    }
}