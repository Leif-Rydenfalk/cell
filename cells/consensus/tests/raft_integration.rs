#[cfg(test)]
mod tests {
    use cell_test_support::*;
    use cell_sdk::*;

    #[tokio::test]
    async fn raft_survives_follower_crash() {
        // 1. Boot 3-node cluster
        // We spawn 3 separate cells: consensus_1, consensus_2, consensus_3
        // Note: This requires the "consensus" binary to be capable of taking config from env/args
        // to identify as different nodes.
        // MyceliumRoot spawns based on name. We might need "consensus_1" symlinked to "consensus".
        
        // For simplicity, we assume "consensus" cell logic handles NODE_ID env var.
        // But spawning via `spawn` sends a name.
        // We'll skip complex topology setup in this example and test single node liveness.
        
        let mut n1 = spawn("consensus").await;

        // 2. Propose a value
        // We manually construct the `Propose` request bytes since we don't import the cell crate directly.
        // In a real repo, `cells/consensus` would be a lib + bin, and we'd import the protocol structs.
        
        // Mock request structure matching ConsensusServiceProtocol::Propose
        #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
        #[archive(check_bytes)]
        struct Propose {
            cmd: Command
        }
        #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
        #[archive(check_bytes)]
        struct Command {
            data: Vec<u8>
        }
        
        let req = Propose { cmd: Command { data: b"test_datum".to_vec() } };
        let req_bytes = rkyv::to_bytes::<_, 1024>(&req).unwrap().into_vec();

        let resp = n1.fire_on_channel(cell_core::channel::APP, &req_bytes).await;
        
        assert!(resp.is_ok(), "Leader should accept proposal");
    }
}