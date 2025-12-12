// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_transport::{Membrane, CoordinationHandler};
use tracing::{info, warn};
use cell_model::macro_coordination::{MacroInfo, ExpansionContext};
use cell_model::protocol::MitosisPhase;
use std::pin::Pin;
use std::future::Future;
use tokio::time::Duration;
use std::io::Write;

pub struct Runtime;

impl Runtime {
    fn emit_signal(phase: MitosisPhase) {
        // We write directly to the raw stdout file descriptor to bypass any 
        // tracing/logging layers that might be active.
        if let Ok(bytes) = cell_model::rkyv::to_bytes::<_, 256>(&phase) {
            let len = bytes.len() as u32;
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(&len.to_le_bytes());
            let _ = stdout.write_all(&bytes);
            let _ = stdout.flush();
        }
    }

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
        // 1. Prophase: Initialization
        Self::emit_signal(MitosisPhase::Prophase);

        // Hook panic to emit Necrosis
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            Self::emit_signal(MitosisPhase::Necrosis);
            default_hook(info);
        }));

        // 2. Hydrate Identity
        let identity = crate::identity::Identity::get();
        let cell_name = identity.cell_name.clone();
        let node_id = identity.node_id;

        info!("[Runtime] Booting Cell '{}' (Node {})", cell_name, node_id);

        // 3. Metaphase: Identity established (Signaled internally by Identity::get return)
        Self::emit_signal(MitosisPhase::Metaphase);

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

        // 4. Prometaphase & Cytokinesis handled inside Membrane::bind or wrapper
        // Since Membrane::bind blocks, we need to know the socket path before it blocks.
        // But Membrane::bind does the bind and accept in one go in the current impl.
        // We will emit Prometaphase just before bind with the *expected* path.
        let socket_dir = crate::resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));
        
        Self::emit_signal(MitosisPhase::Prometaphase { 
            socket_path: socket_path.to_string_lossy().to_string() 
        });

        info!("[Runtime] Membrane binding to {}", cell_name);
        
        // 5. Cytokinesis: We are about to enter the service loop
        Self::emit_signal(MitosisPhase::Cytokinesis);

        match Membrane::bind::<S, Req, Resp>(&cell_name, service, None, None, coordination_handler).await {
            Ok(_) => Ok(()),
            Err(e) => {
                Self::emit_signal(MitosisPhase::Apoptosis { reason: e.to_string() });
                Err(e)
            }
        }
    }
}