// SPDX-License-Identifier: MIT
// cell-sdk/src/system.rs

use crate::Synapse;
use anyhow::{anyhow, Result};
use cell_model::config::CellInitConfig;
use cell_model::protocol::{MitosisRequest, MitosisResponse};

pub struct System;

impl System {
    pub async fn spawn(cell_name: &str, config: Option<CellInitConfig>) -> Result<String> {
        // Fixed: Removed unused 'mut'
        let synapse = Synapse::grow("hypervisor")
            .await
            .map_err(|_| anyhow!("Hypervisor neighbor not found."))?;

        let req = MitosisRequest::Spawn {
            cell_name: cell_name.to_string(),
            config,
        };

        let resp_wrapper = synapse.fire(&req).await?;
        let resp_bytes = resp_wrapper.into_owned();

        if resp_bytes.is_empty() {
            return Err(anyhow!("Empty response from Hypervisor"));
        }

        let archived = cell_model::rkyv::check_archived_root::<MitosisResponse>(&resp_bytes)
            .map_err(|e| anyhow!("Invalid system response: {:?}", e))?;

        let resp: MitosisResponse = cell_model::rkyv::Deserialize::deserialize(
            archived,
            &mut cell_model::rkyv::de::deserializers::SharedDeserializeMap::new(),
        )?;

        match resp {
            MitosisResponse::Ok { socket_path } => Ok(socket_path),
            MitosisResponse::Denied { reason } => Err(anyhow!("Spawn denied: {}", reason)),
        }
    }

    pub async fn ignite_local_cluster() -> Result<()> {
        // Placeholder for future implementation
        Ok(())
    }
}
