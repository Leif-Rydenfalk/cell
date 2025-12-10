// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_model::macro_coordination::*;
use std::time::Duration;
use cell_transport::Synapse;

pub struct MacroCoordinator {
    cell_name: String,
}

impl MacroCoordinator {
    pub fn new(cell_name: &str) -> Self {
        Self {
            cell_name: cell_name.to_string(),
        }
    }

    pub fn connect_and_query(&self, request: MacroCoordinationRequest) -> Result<MacroCoordinationResponse> {
        // Create runtime and block on async operation
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        rt.block_on(async {
            // Try to connect with timeout
            let connect_result = tokio::time::timeout(
                Duration::from_secs(2),
                self.try_connect()
            ).await;

            match connect_result {
                Ok(Ok(mut synapse)) => {
                    // Serialize request
                    let req_bytes = rkyv::to_bytes::<_, 1024>(&request)?.into_vec();
                    
                    // Send on MACRO_COORDINATION channel
                    let response = synapse.fire_on_channel(
                        cell_core::channel::MACRO_COORDINATION,
                        &req_bytes
                    ).await?;

                    // Deserialize response
                    let resp = response.deserialize()?;
                    Ok(resp)
                }
                Ok(Err(e)) => {
                    // Connection failed - use cached/fallback
                    Ok(MacroCoordinationResponse::Error {
                        message: format!("Cell '{}' not running: {}", self.cell_name, e)
                    })
                }
                Err(_) => {
                    // Timeout
                    Ok(MacroCoordinationResponse::Error {
                        message: format!("Cell '{}' connection timeout", self.cell_name)
                    })
                }
            }
        })
    }

    async fn try_connect(&self) -> Result<Synapse> {
        Synapse::grow(&self.cell_name).await
    }

    pub fn query_macros(&self) -> Result<Vec<MacroInfo>> {
        let response = self.connect_and_query(
            MacroCoordinationRequest::WhatMacrosDoYouProvide
        )?;

        match response {
            MacroCoordinationResponse::Macros { macros } => Ok(macros),
            MacroCoordinationResponse::Error { message } => {
                // Fallback: check cached macros
                // In a real implementation we would log this warning
                eprintln!("Warning: Failed to query macros from {}: {}", self.cell_name, message);
                Ok(self.get_cached_macros()?)
            }
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn coordinate_expansion(
        &self,
        macro_name: &str,
        context: ExpansionContext,
    ) -> Result<String> {
        let response = self.connect_and_query(
            MacroCoordinationRequest::CoordinateExpansion {
                macro_name: macro_name.to_string(),
                context,
            }
        )?;

        match response {
            MacroCoordinationResponse::GeneratedCode { code } => Ok(code),
            MacroCoordinationResponse::Error { message } => {
                anyhow::bail!("Coordination failed: {}", message)
            }
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    fn get_cached_macros(&self) -> Result<Vec<MacroInfo>> {
        // Check ~/.cell/macros/{cell_name}/manifest.json
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home dir"))?;
        let manifest_path = home
            .join(".cell/macros")
            .join(&self.cell_name)
            .join("manifest.json");

        if !manifest_path.exists() {
            return Ok(vec![]);
        }

        let content = std::fs::read_to_string(manifest_path)?;
        let macros: Vec<MacroInfo> = serde_json::from_str(&content)?;
        Ok(macros)
    }
}