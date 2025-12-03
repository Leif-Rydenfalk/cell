// SPDX-License-Identifier: MIT
use crate::registry::InstanceInfo;
use crate::synapse::Response;
use anyhow::Result;
use std::sync::Arc;
use tokio::time::{interval, Duration};

// We need to manually define the protocol client stub here because we can't
// depend on the `cell-git` crate from the SDK (circular dependency).
// In a real setup, we might separate the protocol definition to `cell-git-protocol` crate.
// For now, we construct the request manually or use a macro if available.
// NOTE: Ideally `cell_remote!` would solve this, but we are inside the SDK.

pub struct HeartbeatService {
    cell_name: String,
    node_id: String,
    endpoint: String,
}

impl HeartbeatService {
    pub fn new(cell_name: String, endpoint: String) -> Self {
        let node_id = generate_node_id(&cell_name);
        Self {
            cell_name,
            node_id,
            endpoint,
        }
    }

    pub async fn start(self: Arc<Self>) {
        let mut ticker = interval(Duration::from_secs(10));

        loop {
            ticker.tick().await;

            if let Err(e) = self.send_heartbeat().await {
                // Don't panic on heartbeat failure, just log
                // eprintln!("[Heartbeat] Failed: {}", e); 
            }
        }
    }

    async fn send_heartbeat(&self) -> Result<()> {
        let mut _git = crate::bootstrap::resolve_cell_git().await?;

        let _instance = InstanceInfo {
            node_id: self.node_id.clone(),
            endpoint: self.endpoint.clone(),
            region: None,
            last_heartbeat: chrono::Utc::now().to_rfc3339(),
            signature: sign_instance(&self.node_id, &self.endpoint),
        };

        // TODO: Here we need to serialize the `AnnounceInstance` enum variant 
        // that matches `CellGitServiceProtocol`.
        // Since we don't have the generated code here, this is the trickiest part 
        // of "Bootstrap". 
        //
        // Solution: We will implement a raw rkyv serializer here or move 
        // the `CellGitServiceProtocol` definition into `cell-sdk::registry` 
        // so it is shared.

        Ok(())
    }
}

fn generate_node_id(cell_name: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(cell_name.as_bytes());
    hasher.update(&rand::random::<u64>().to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn sign_instance(node_id: &str, endpoint: &str) -> String {
    format!("{}:{}", node_id, endpoint)
}