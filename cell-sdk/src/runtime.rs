// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, Context};
use cell_transport::Membrane;
use crate::config::CellConfig;
use tracing::info;

pub struct Runtime;

impl Runtime {
    pub async fn ignite<S, Req, Resp>(
        service: S, 
        name: &str,
    ) -> Result<()>
    where
        S: for<'a> Fn(&'a Req::Archived) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Resp>> + Send + 'a>>
            + Send + Sync + 'static + Clone,
        Req: cell_model::rkyv::Archive + Send,
        Req::Archived: for<'a> cell_model::rkyv::CheckBytes<cell_model::rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<1024>> + Send + 'static,
    {
        let config = CellConfig::from_env(name).context("Failed to load Cell configuration")?;
        info!("[Runtime] Booting Cell '{}' (Node {})", name, config.node_id);

        #[cfg(feature = "axon")]
        {
            let _ = cell_axon::pheromones::PheromoneSystem::ignite(config.node_id).await?;
        }

        info!("[Runtime] Membrane binding to {}", name);
        Membrane::bind::<S, Req, Resp>(name, service, None, None).await
    }
}