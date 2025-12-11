#[cfg(test)]
mod tests {
    use cell_test_support::*;
    use cell_sdk::*;

    #[tokio::test]
    async fn raft_survives_follower_crash() {
        let mut n1 = spawn("consensus").await;

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

        // Fixed import: cell_sdk::channel instead of cell_core::channel if re-exported
        // or ensure cell_core is accessible. 
        // cell-sdk re-exports cell_core content at top level.
        // so cell_sdk::channel should work.
        let resp = n1.fire_on_channel(cell_sdk::channel::APP, &req_bytes).await;
        
        assert!(resp.is_ok(), "Leader should accept proposal");
    }
}