// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_model::macro_coordination::*;
use anyhow::Result;
use std::sync::Arc;
use std::pin::Pin;
use std::future::Future;
use std::time::Duration;
use crate::synapse::Synapse;
use crate::response::Response;
use rkyv::Deserialize;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Server-side handler for Macro Coordination
pub struct CoordinationHandler {
    #[allow(dead_code)]
    cell_name: String,
    macros: Vec<MacroInfo>,
    expander: Box<dyn Fn(&str, &ExpansionContext) -> BoxFuture<'static, Result<String>> + Send + Sync>,
}

impl CoordinationHandler {
    pub fn new<F>(cell_name: &str, macros: Vec<MacroInfo>, expander: F) -> Arc<Self> 
    where F: Fn(&str, &ExpansionContext) -> BoxFuture<'static, Result<String>> + Send + Sync + 'static
    {
        Arc::new(Self {
            cell_name: cell_name.to_string(),
            macros,
            expander: Box::new(expander),
        })
    }

    pub async fn handle(
        &self,
        request: &ArchivedMacroCoordinationRequest,
    ) -> Result<MacroCoordinationResponse> {
        let req: MacroCoordinationRequest = request
            .deserialize(&mut cell_model::rkyv::de::deserializers::SharedDeserializeMap::new())?;

        match req {
            MacroCoordinationRequest::WhatMacrosDoYouProvide => {
                Ok(MacroCoordinationResponse::Macros {
                    macros: self.macros.clone(),
                })
            }
            MacroCoordinationRequest::GetMacroInfo { name } => {
                let info = self.macros.iter()
                    .find(|m| m.name == name)
                    .cloned();
                
                match info {
                    Some(info) => Ok(MacroCoordinationResponse::MacroInfo { info }),
                    None => Ok(MacroCoordinationResponse::Error {
                        message: format!("Macro '{}' not found", name),
                    }),
                }
            }
            MacroCoordinationRequest::CoordinateExpansion { macro_name, context } => {
                match (self.expander)(&macro_name, &context).await {
                    Ok(code) => Ok(MacroCoordinationResponse::GeneratedCode { code }),
                    Err(e) => Ok(MacroCoordinationResponse::Error {
                        message: e.to_string(),
                    }),
                }
            }
            MacroCoordinationRequest::QueryOtherCell { target_cell, query } => {
                self.query_other_cell(&target_cell, &query).await
            }
        }
    }

    async fn query_other_cell(&self, target: &str, query: &str) -> Result<MacroCoordinationResponse> {
        let mut synapse = Synapse::grow(target).await?;
        
        let request = MacroCoordinationRequest::QueryOtherCell {
            target_cell: target.to_string(),
            query: query.to_string(),
        };

        let req_bytes = cell_model::rkyv::to_bytes::<_, 1024>(&request)?.into_vec();
        
        // Handle CellError -> anyhow::Error mapping manually since ? might fail with mismatched types
        let response = match synapse.fire_on_channel(
            cell_core::channel::MACRO_COORDINATION,
            &req_bytes
        ).await {
            Ok(r) => r,
            Err(e) => anyhow::bail!("Transport error: {}", e),
        };

        let bytes = match &response {
            Response::Owned(vec) => vec.as_slice(),
            Response::Borrowed(slice) => slice,
            _ => anyhow::bail!("Unexpected response type for macro coordination"),
        };
        
        let archived = cell_model::rkyv::check_archived_root::<MacroCoordinationResponse>(bytes)
            .map_err(|e| anyhow::anyhow!("Invalid response from other cell: {:?}", e))?;
        
        Ok(archived.deserialize(&mut cell_model::rkyv::de::deserializers::SharedDeserializeMap::new())?)
    }
}

/// Client-side coordinator for interacting with Cell Macros
pub struct MacroCoordinator {
    cell_name: String,
}

impl MacroCoordinator {
    pub fn new(cell_name: &str) -> Self {
        Self {
            cell_name: cell_name.to_string(),
        }
    }

    /// Helper to run the async query in a blocking context (for use inside proc macros)
    pub fn connect_and_query(&self, request: MacroCoordinationRequest) -> Result<MacroCoordinationResponse> {
        // Create runtime and block on async operation
        // Note: In a real environment, reusing a global runtime is better, but proc-macros are short-lived.
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
                    let req_bytes = rkyv::to_bytes::<_, 1024>(&request)?.into_vec();
                    
                    let response = match synapse.fire_on_channel(
                        cell_core::channel::MACRO_COORDINATION,
                        &req_bytes
                    ).await {
                        Ok(r) => r,
                        Err(e) => anyhow::bail!("Transport error: {}", e),
                    };

                    let bytes = match &response {
                        Response::Owned(vec) => vec.as_slice(),
                        Response::Borrowed(slice) => slice,
                        _ => anyhow::bail!("Unexpected response type for macro coordination"),
                    };

                    let archived = cell_model::rkyv::check_archived_root::<MacroCoordinationResponse>(bytes)
                        .map_err(|e| anyhow::anyhow!("Invalid coordination response: {:?}", e))?;
                    
                    let resp = archived.deserialize(&mut cell_model::rkyv::de::deserializers::SharedDeserializeMap::new())?;
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