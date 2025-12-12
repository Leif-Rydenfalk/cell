// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_transport::{Membrane, CoordinationHandler};
use tracing::{info, warn};
use cell_model::macro_coordination::{MacroInfo, ExpansionContext};
use cell_model::protocol::MitosisSignal;
use std::pin::Pin;
use std::future::Future;
use tokio::time::Duration;

pub struct Runtime;

impl Runtime {
    fn emit_signal(signal: MitosisSignal) {
        // Access the global Gap Junction established during Identity hydration
        if let Some(mutex) = crate::identity::GAP_JUNCTION.get() {
            if let Ok(mut junction) = mutex.lock() {
                // If this fails (broken pipe), we probably can't do much but log or die.
                let _ = junction.signal(signal);
            }
        } else {
            // If we are running standalone (no hypervisor), we might not have a junction.
            // Just ignore signals in that case, or log to stderr.
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
        // 1. Prophase: Initialization (Before Identity)
        // We can't use emit_signal yet because Identity hasn't run to open the FD.
        // However, we can try to open it early if we wanted, but Identity handles it safely.
        // Let's defer Prophase signal to *after* Identity bootstrap inside `Identity::get()`?
        // No, Identity needs to happen first.
        
        // Hook panic to emit Necrosis
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            Self::emit_signal(MitosisSignal::Necrosis);
            default_hook(info);
        }));

        // 2. Hydrate Identity (This performs the Handshake: RequestIdentity -> Receive Config)
        let identity = crate::identity::Identity::get();
        let cell_name = identity.cell_name.clone();
        let node_id = identity.node_id;

        // Now we have the Junction.
        Self::emit_signal(MitosisSignal::Prophase);

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

        // 3. Prometaphase
        let socket_dir = crate::resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));
        
        Self::emit_signal(MitosisSignal::Prometaphase { 
            socket_path: socket_path.to_string_lossy().to_string() 
        });

        info!("[Runtime] Membrane binding to {}", cell_name);
        
        // 4. Cytokinesis: We are about to enter the service loop
        Self::emit_signal(MitosisSignal::Cytokinesis);

        match Membrane::bind::<S, Req, Resp>(&cell_name, service, None, None, coordination_handler).await {
            Ok(_) => Ok(()),
            Err(e) => {
                Self::emit_signal(MitosisSignal::Apoptosis { reason: e.to_string() });
                Err(e)
            }
        }
    }
}