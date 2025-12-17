// SPDX-License-Identifier: MIT
// cell-sdk/src/runtime.rs

use crate::identity::Identity;
use crate::membrane::Membrane;
use crate::mesh::MeshBuilder;
use crate::organogenisis::Organism;
use anyhow::{Context, Result};
use cell_model::macro_coordination::{ExpansionContext, MacroInfo};
use std::future::Future;
use std::pin::Pin;
use tracing::{info, warn};

pub struct Runtime;

impl Runtime {
    /// Entry point for cells with dependencies.
    pub async fn ignite_with_deps<S, Req, Resp>(service: S, name: &str, deps: &[&str]) -> Result<()>
    where
        S: for<'a> Fn(&'a Req::Archived) -> Pin<Box<dyn Future<Output = Result<Resp>> + Send + 'a>>
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
        // 1. Declare Dependencies
        let deps_vec: Vec<String> = deps.iter().map(|s| s.to_string()).collect();
        MeshBuilder::declare_dependencies(name, deps_vec).await;

        // 2. Wait for Dependencies (Check neighbor paths)
        MeshBuilder::wait_for_dependencies(deps).await?;

        Self::ignite::<S, Req, Resp>(service, name).await
    }

    /// Standard entry point.
    pub async fn ignite<S, Req, Resp>(service: S, name: &str) -> Result<()>
    where
        S: for<'a> Fn(&'a Req::Archived) -> Pin<Box<dyn Future<Output = Result<Resp>> + Send + 'a>>
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

    /// Advanced entry point with Macro Coordination.
    pub async fn ignite_with_coordination<S, Req, Resp, F>(
        service: S,
        name: &str,
        macros: Vec<MacroInfo>,
        _expander: F, // Unused in runtime for now, used by build tools
    ) -> Result<()>
    where
        S: for<'a> Fn(&'a Req::Archived) -> Pin<Box<dyn Future<Output = Result<Resp>> + Send + 'a>>
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
        // 1. Setup Global Error Handling
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!("Cell Panic: {:?}", info);
            default_hook(info);
        }));

        // 2. Load Identity (Env / Config)
        let config = Identity::get();
        info!(
            "[Runtime] Booting Cell '{}' (Node {})",
            config.cell_name, config.node_id
        );

        // 3. Organogenesis (Build Filesystem Structure)
        Organism::develop().context("Failed to build cell anatomy")?;

        // 4. Register with Mesh (File-based registry)
        if let Err(e) = MeshBuilder::announce_self(name).await {
            warn!("[Runtime] Failed to announce self to mesh: {}", e);
        }

        // 5. Macro Coordination Handler
        // If this cell provides macros, we might need to listen on a specific channel.
        // For simplicity in this FS architecture, we pass the info to the Membrane.
        let coordination_ctx = if !macros.is_empty() {
            Some(macros) // Membrane can expose this via metadata endpoint
        } else {
            None
        };

        info!("[Runtime] Membrane binding to io/in");

        // 6. Start the Membrane (Main Loop)
        // Note: Membrane::bind loop is infinite
        Membrane::bind::<S, Req, Resp>(
            name,
            service,
            None,
            None,
            coordination_ctx.map(|_| ()), // Type erasure for now
        )
        .await
    }
}
