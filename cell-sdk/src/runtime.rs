// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_transport::{Membrane, CoordinationHandler};
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
        _name: &str, 
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
        // 1. Hydrate Identity from Umbilical Cord (blocks if needed)
        let identity = crate::identity::Identity::get();
        let cell_name = identity.cell_name.clone();
        let node_id = identity.node_id;

        info!("[Runtime] Booting Cell '{}' (Node {})", cell_name, node_id);

        // Start background registration with Nucleus
        let reg_name = cell_name.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            
            let mut backoff = Duration::from_secs(2);
            loop {
                match crate::NucleusClient::connect().await {
                    Ok(mut nucleus) => {
                        match nucleus.register(reg_name.clone(), node_id).await {
                            Ok(_) => {
                                info!("[Runtime] Registered '{}' with Nucleus", reg_name);
                                loop {
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                    if let Err(e) = nucleus.register(reg_name.clone(), node_id).await {
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
            Some(CoordinationHandler::new(&cell_name, macros, expander))
        } else {
            None
        };

        info!("[Runtime] Membrane binding to {}", cell_name);
        Membrane::bind::<S, Req, Resp>(&cell_name, service, None, None, coordination_handler).await
    }
}