// cell-sdk/src/router.rs
// The Nervous System: User-space packet switching.

use anyhow::Result;
use cell_core::vesicle::VesicleHeader;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::RwLock;
use tracing::{trace, warn};

pub struct NervousSystem {
    // Hash of service name -> Neighbor name
    routes: Arc<RwLock<HashMap<u64, String>>>,
}

impl NervousSystem {
    pub fn new() -> Self {
        Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Handles a packet that is NOT for us.
    pub async fn forward(&self, mut header: VesicleHeader, payload: &[u8]) -> Result<()> {
        if header.ttl == 0 {
            warn!("Packet dropped: TTL expired");
            return Ok(());
        }

        header.ttl -= 1;

        // 1. Check direct neighbors (optimization: check if target hash matches a neighbor name hash)
        // For MVP, we route everything unknown to 'default' (Gateway) if it exists.

        let target_sock = if let Some(neighbor) = self.lookup_route(header.target_id).await {
            // Known route
            std::env::current_dir()?
                .join(".cell/neighbors")
                .join(neighbor)
        } else {
            // Default Gateway
            std::env::current_dir()?.join(".cell/neighbors/default")
        };

        if target_sock.exists() {
            let mut stream = UnixStream::connect(target_sock).await?;

            // Re-serialize Header + Payload
            // Note: In a real impl, we'd use a struct-to-bytes method
            let header_bytes = unsafe {
                std::slice::from_raw_parts(
                    &header as *const _ as *const u8,
                    std::mem::size_of::<VesicleHeader>(),
                )
            };

            // Frame: [0x02 (ROUTED)] [Header] [Payload]
            stream.write_u8(0x02).await?;
            stream.write_all(header_bytes).await?;
            stream.write_all(payload).await?;

            // We don't wait for response here in the background router;
            // In a req/resp model, the response finds its way back via source_id.
            // For simple RPC, we might hold the stream open.
            // Simplified: The connection handles the return path.
        } else {
            warn!(
                "Network unreachable: No route to target {:x}",
                header.target_id
            );
        }

        Ok(())
    }

    async fn lookup_route(&self, _target_id: u64) -> Option<String> {
        // Implement routing table lookup here
        None
    }
}
