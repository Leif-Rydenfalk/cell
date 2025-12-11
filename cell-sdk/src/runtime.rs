// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, Context};
use cell_transport::{Membrane, CoordinationHandler};
use crate::config::CellConfig;
use tracing::{info, warn};
use cell_model::macro_coordination::{MacroInfo, ExpansionContext};
use std::pin::Pin;
use std::future::Future;
use tokio::time::Duration;

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

        // NOTE: Pheromone discovery is now handled by the external Axon cell.
        // The runtime focuses purely on local serving and nucleus registration.

        // Start background registration with Nucleus
        let cell_name = name.to_string();
        let node_id = config.node_id;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            
            let mut backoff = Duration::from_secs(2);
            loop {
                match crate::NucleusClient::connect().await {
                    Ok(mut nucleus) => {
                        match nucleus.register(cell_name.clone(), node_id).await {
                            Ok(_) => {
                                info!("[Runtime] Registered '{}' with Nucleus", cell_name);
                                loop {
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                    if let Err(e) = nucleus.register(cell_name.clone(), node_id).await {
                                        warn!("[Runtime] Nucleus heartbeat failed: {}", e);
                                        break; 
                                    }
                                }
                            },
                            Err(e) => warn!("[Runtime] Registration failed: {}", e),
                        }
                    },
                    Err(_) => {}
                }
                tokio::time::sleep(backoff).await;
                if backoff < Duration::from_secs(60) { backoff *= 2; }
            }
        });

        let coordination_handler = if !macros.is_empty() {
            Some(CoordinationHandler::new(name, macros, expander))
        } else {
            None
        };

        info!("[Runtime] Membrane binding to {}", name);
        Membrane::bind::<S, Req, Resp>(name, service, None, None, coordination_handler).await
    }
}