// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, Context};
use cell_transport::{Membrane, CoordinationHandler};
use crate::config::CellConfig;
use tracing::info;
use cell_model::macro_coordination::{MacroInfo, ExpansionContext};
use std::pin::Pin;
use std::future::Future;

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
        // FIX: Added explicit type parameters <S, Req, Resp, _>
        Self::ignite_with_coordination::<S, Req, Resp, _>(
            service, 
            name, 
            vec![], 
            |_, _| Box::pin(async { Ok(String::new()) })
        ).await
    }

    pub async fn ignite_with_coordination<S, Req, Resp, F>(
        service: S,
        name: &str,
        macros: Vec<MacroInfo>,
        expander: F,
    ) -> Result<()>
    where
        S: for<'a> Fn(&'a Req::Archived) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Resp>> + Send + 'a>>
            + Send + Sync + 'static + Clone,
        Req: cell_model::rkyv::Archive + Send,
        Req::Archived: for<'a> cell_model::rkyv::CheckBytes<cell_model::rkyv::validation::validators::DefaultValidator<'a>> + 'static,
        Resp: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<1024>> + Send + 'static,
        F: Fn(&str, &ExpansionContext) -> Pin<Box<dyn Future<Output = Result<String>> + Send>> + Send + Sync + 'static,
    {
        let config = CellConfig::from_env(name).context("Failed to load Cell configuration")?;
        info!("[Runtime] Booting Cell '{}' (Node {})", name, config.node_id);

        #[cfg(feature = "axon")]
        {
            let _ = cell_axon::pheromones::PheromoneSystem::ignite(config.node_id).await?;
        }

        let coordination_handler = if !macros.is_empty() {
            Some(CoordinationHandler::new(name, macros, expander))
        } else {
            None
        };

        info!("[Runtime] Membrane binding to {}", name);
        Membrane::bind::<S, Req, Resp>(name, service, None, None, coordination_handler).await
    }
}