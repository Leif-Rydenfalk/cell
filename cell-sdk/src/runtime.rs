// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, Context};
use cell_transport::{Membrane, Synapse};
use cell_model::protocol::CellGenome;
use crate::config::CellConfig;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, error};

#[cfg(feature = "consensus")]
use cell_consensus::{RaftConfig, RaftNode, StateMachine};

pub struct Runtime;

impl Runtime {
    pub async fn ignite<S, Req, Resp>(
        service: S, 
        name: &str,
        raft_sm: Option<Arc<dyn StateMachine>>,
    ) -> Result<()>
    where
        S: for<'a> Fn(&'a Req::Archived) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Resp>> + Send + 'a>>
            + Send + Sync + 'static + Clone,
        Req: cell_model::rkyv::Archive + Send,
        Req::Archived: for<'a> cell_model::rkyv::CheckBytes<cell_model::rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<1024>> + Send + 'static,
    {
        // 0. Load Configuration
        let config = CellConfig::from_env(name).context("Failed to load Cell configuration")?;
        info!("[Runtime] Booting Cell '{}' (Node {})", name, config.node_id);

        #[cfg(feature = "axon")]
        let _ = cell_axon::PheromoneSystem::ignite().await?;

        let consensus_tx = if let Some(sm) = raft_sm {
            #[cfg(feature = "consensus")]
            {
                info!("[Runtime] Initializing Consensus Layer...");
                let (tx, rx) = mpsc::channel(1000);
                
                let raft_config = RaftConfig {
                    id: config.node_id,
                    storage_path: config.raft_storage_path.ok_or_else(|| anyhow::anyhow!("Raft storage path not configured"))?,
                    peers: config.peers,
                };

                tokio::spawn(async move {
                    if let Err(e) = RaftNode::ignite(raft_config, sm, rx).await {
                        error!("[Runtime] Raft died: {}", e);
                    }
                });

                Some(tx)
            }
            #[cfg(not(feature = "consensus"))]
            {
                tracing::warn!("Consensus requested but feature disabled");
                None
            }
        } else {
            None
        };

        info!("[Runtime] Membrane binding to {}", name);
        Membrane::bind(name, service, None, consensus_tx).await
    }
}