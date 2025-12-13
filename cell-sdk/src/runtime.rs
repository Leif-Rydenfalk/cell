// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_transport::{Membrane, CoordinationHandler, Synapse};
use cell_core::CellError;
use tracing::{info, warn};
use cell_model::macro_coordination::{MacroInfo, ExpansionContext};
use cell_model::protocol::MitosisSignal;
use std::pin::Pin;
use std::future::Future;
use tokio::time::Duration;

// --- Internal Nucleus Client for Registration ---
// Defined locally to avoid circular build dependency on 'nucleus' cell
mod nucleus_internal {
    use super::*;
    use cell_macros::protein;
    // Removed unused import
    use cell_model::rkyv::Deserialize; 

    #[protein]
    pub struct CellRegistration {
        pub name: String,
        pub node_id: u64,
        pub capabilities: Vec<String>,
        pub endpoints: Vec<String>,
    }

    #[derive(serde::Serialize, serde::Deserialize, cell_model::rkyv::Archive, cell_model::rkyv::Serialize, cell_model::rkyv::Deserialize, Debug, Clone)]
    #[archive(check_bytes)]
    #[archive(crate = "cell_model::rkyv")]
    #[serde(crate = "serde")]
    pub enum NucleusProtocol {
        Register(CellRegistration),
        // other variants ignored
    }

    pub struct Client {
        conn: Synapse,
    }

    impl Client {
        pub async fn connect() -> Result<Self> {
            let conn = Synapse::grow("nucleus").await?;
            Ok(Self { conn })
        }

        pub async fn register(&mut self, name: String, node_id: u64) -> Result<bool, CellError> {
            let reg = CellRegistration {
                name,
                node_id,
                capabilities: vec![],
                endpoints: vec![],
            };
            
            #[derive(serde::Serialize, serde::Deserialize, cell_model::rkyv::Archive, cell_model::rkyv::Serialize, cell_model::rkyv::Deserialize, Debug, Clone)]
            #[archive(check_bytes)]
            #[archive(crate = "cell_model::rkyv")]
            #[serde(crate = "serde")]
            enum NucleusResponse {
                Register(bool),
                // others ignored
            }

            let req = NucleusProtocol::Register(reg);
            let resp = self.conn.fire::<NucleusProtocol, NucleusResponse>(&req).await?;
            
            match resp.deserialize().map_err(|_| CellError::SerializationFailure)? {
                NucleusResponse::Register(b) => Ok(b),
            }
        }
    }
}

pub struct Runtime;

impl Runtime {
    fn emit_signal(signal: MitosisSignal) {
        if let Some(mutex) = crate::identity::GAP_JUNCTION.get() {
            if let Ok(mut junction) = mutex.lock() {
                let _ = junction.signal(signal);
            }
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
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            Self::emit_signal(MitosisSignal::Necrosis);
            default_hook(info);
        }));

        let identity = crate::identity::Identity::get();
        let cell_name = identity.cell_name.clone();
        let node_id = identity.node_id;

        Self::emit_signal(MitosisSignal::Prophase);

        info!("[Runtime] Booting Cell '{}' (Node {})", cell_name, node_id);

        let reg_name = cell_name.clone();
        
        // Background Heartbeat Task
        tokio::spawn(async move {
            // Wait for transport to initialize
            tokio::time::sleep(Duration::from_secs(1)).await;
            
            let mut backoff = Duration::from_secs(2);
            loop {
                // Use the internal manual client
                match nucleus_internal::Client::connect().await {
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

        let socket_dir = crate::resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));
        
        Self::emit_signal(MitosisSignal::Prometaphase { 
            socket_path: socket_path.to_string_lossy().to_string() 
        });

        info!("[Runtime] Membrane binding to {}", cell_name);
        
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