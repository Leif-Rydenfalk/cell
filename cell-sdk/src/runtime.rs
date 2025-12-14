// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_core::CellError;
use cell_model::macro_coordination::{ExpansionContext, MacroInfo};
use cell_model::protocol::MitosisSignal;
use cell_transport::{CoordinationHandler, Membrane, Synapse};
use std::future::Future;
use std::pin::Pin;
use tokio::time::Duration;
use tracing::{info, warn};

pub struct Runtime;

impl Runtime {
    fn emit_signal(signal: MitosisSignal) {
        if let Some(mutex) = crate::identity::GAP_JUNCTION.get() {
            if let Ok(mut junction) = mutex.lock() {
                let _ = junction.signal(signal);
            }
        }
    }

    pub async fn ignite<S, Req, Resp>(service: S, name: &str) -> Result<()>
    where
        S: for<'a> Fn(
                &'a Req::Archived,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<Resp>> + Send + 'a>,
            >
            + Send
            + Sync
            + 'static
            + Clone,
        Req: cell_model::rkyv::Archive + Send,
        Req::Archived: for<'a> cell_model::rkyv::CheckBytes<
                cell_model::rkyv::validation::validators::DefaultValidator<'a>,
            > + 'static,
        Resp: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<1024>>
            + Send
            + 'static,
    {
        Self::ignite_with_coordination::<S, Req, Resp, _>(service, name, vec![], |_, _| {
            Box::pin(async { Ok(String::new()) })
        })
        .await
    }

    pub async fn ignite_with_coordination<S, Req, Resp, F>(
        service: S,
        _name: &str,
        macros: Vec<MacroInfo>,
        expander: F,
    ) -> Result<()>
    where
        S: for<'a> Fn(
                &'a Req::Archived,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<Resp>> + Send + 'a>,
            >
            + Send
            + Sync
            + 'static
            + Clone,
        Req: cell_model::rkyv::Archive + Send,
        Req::Archived: for<'a> cell_model::rkyv::CheckBytes<
                cell_model::rkyv::validation::validators::DefaultValidator<'a>,
            > + 'static,
        Resp: cell_model::rkyv::Serialize<cell_model::rkyv::ser::serializers::AllocSerializer<1024>>
            + Send
            + 'static,
        F: Fn(&str, &ExpansionContext) -> Pin<Box<dyn Future<Output = Result<String>> + Send>>
            + Send
            + Sync
            + 'static,
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

        let coordination_handler = if !macros.is_empty() {
            Some(CoordinationHandler::new(&cell_name, macros, expander))
        } else {
            None
        };

        let socket_dir = crate::resolve_socket_dir();
        let socket_path = socket_dir.join(format!("{}.sock", cell_name));

        Self::emit_signal(MitosisSignal::Prometaphase {
            socket_path: socket_path.to_string_lossy().to_string(),
        });

        info!("[Runtime] Membrane binding to {}", cell_name);

        Self::emit_signal(MitosisSignal::Cytokinesis);

        match Membrane::bind::<S, Req, Resp>(&cell_name, service, None, None, coordination_handler)
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                Self::emit_signal(MitosisSignal::Apoptosis {
                    reason: e.to_string(),
                });
                Err(e)
            }
        }
    }
}
