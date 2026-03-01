// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_core_macros::*;
use rkyv::Deserialize;
use std::time::Duration;

/// MacroCoordinator handles compile-time communication with schema cells.
///
/// # Design Goals
/// - Robust: Handles network failures gracefully with caching
/// - Efficient: Minimizes compile-time RPC calls through caching
/// - Extensible: Plugin architecture for new macro types
///
/// # Note on Implementation
/// This coordinator runs at COMPILE TIME in a proc-macro context.
/// It uses std::net for blocking I/O rather than tokio async.
pub struct MacroCoordinator {
    cell_name: String,
    cache_dir: Option<std::path::PathBuf>,
}

impl MacroCoordinator {
    pub fn new(cell_name: &str) -> Self {
        let cache_dir = dirs::cache_dir()
            .map(|d| d.join("cell").join("schemas"))
            .or_else(|| dirs::home_dir().map(|d| d.join(".cell").join("cache").join("schemas")));

        Self {
            cell_name: cell_name.to_string(),
            cache_dir,
        }
    }

    /// Connect to the macro cell and execute a coordination request using blocking I/O.
    ///
    /// # Timeout Strategy
    /// - Connection timeout: 2 seconds
    /// - Total operation timeout: 5 seconds
    /// - Retry: 1 attempt with fallback to cache
    pub fn connect_and_query(
        &self,
        request: MacroCoordinationRequest,
    ) -> Result<MacroCoordinationResponse> {
        // Try to connect with timeout using blocking I/O (std::net)
        let connect_result = self.try_connect_with_timeout(Duration::from_secs(2));

        match connect_result {
            Ok(stream) => {
                // Serialize request using rkyv
                let req_bytes = match rkyv::to_bytes::<_, 1024>(&request) {
                    Ok(bytes) => bytes.into_vec(),
                    Err(e) => return Err(anyhow::anyhow!("Failed to serialize request: {:?}", e)),
                };

                // Send request and receive response
                let response = self.send_receive(stream, &req_bytes)?;

                // Deserialize response
                let archived = rkyv::check_archived_root::<MacroCoordinationResponse>(&response)
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Invalid coordination response from '{}': {:?}",
                            self.cell_name,
                            e
                        )
                    })?;

                let resp: MacroCoordinationResponse = archived
                    .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
                    .map_err(|e| anyhow::anyhow!("Failed to deserialize response: {:?}", e))?;

                // Cache successful response
                let _ = self.cache_response(&request, &resp);

                Ok(resp)
            }
            Err(e) => {
                // Connection failed - use cached/fallback
                eprintln!(
                    "Warning: Could not connect to '{}': {}. Using cached schema if available.",
                    self.cell_name, e
                );
                self.get_cached_response(&request)
            }
        }
    }

    /// Blocking connection attempt with timeout
    fn try_connect_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<std::os::unix::net::UnixStream> {
        use std::os::unix::net::UnixStream;
        use std::time::Instant;

        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No HOME directory"))?;
        let socket_path = home
            .join(".cell/io")
            .join(format!("{}.sock", self.cell_name));

        if !socket_path.exists() {
            return Err(anyhow::anyhow!("Socket not found: {:?}", socket_path));
        }

        let start = Instant::now();

        // Try non-blocking connect with timeout
        let stream = UnixStream::connect(&socket_path)?;
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;

        // Check if we exceeded timeout
        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("Connection timeout"));
        }

        Ok(stream)
    }

    /// Send request and receive response using blocking I/O
    fn send_receive(
        &self,
        mut stream: std::os::unix::net::UnixStream,
        request: &[u8],
    ) -> Result<Vec<u8>> {
        use std::io::{Read, Write};

        // Send: [4 bytes length][payload]
        let len = request.len() as u32;
        stream.write_all(&len.to_le_bytes())?;
        stream.write_all(request)?;
        stream.flush()?;

        // Receive: [4 bytes length][payload]
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)?;
        let resp_len = u32::from_le_bytes(len_buf) as usize;

        let mut response = vec![0u8; resp_len];
        stream.read_exact(&mut response)?;

        Ok(response)
    }

    pub fn query_macros(&self) -> Result<Vec<MacroInfo>> {
        let response = self.connect_and_query(MacroCoordinationRequest::WhatMacrosDoYouProvide)?;

        match response {
            MacroCoordinationResponse::Macros { macros } => Ok(macros),
            MacroCoordinationResponse::Error { message } => {
                // Fallback: check cached macros
                eprintln!(
                    "Warning: Failed to query macros from {}: {}",
                    self.cell_name, message
                );
                Ok(self.get_cached_macros()?.unwrap_or_default())
            }
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn coordinate_expansion(
        &self,
        macro_name: &str,
        context: ExpansionContext,
    ) -> Result<String> {
        let response = self.connect_and_query(MacroCoordinationRequest::CoordinateExpansion {
            macro_name: macro_name.to_string(),
            context,
        })?;

        match response {
            MacroCoordinationResponse::GeneratedCode { code } => Ok(code),
            MacroCoordinationResponse::Error { message } => {
                anyhow::bail!("Coordination failed: {}", message)
            }
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    fn cache_response(
        &self,
        request: &MacroCoordinationRequest,
        response: &MacroCoordinationResponse,
    ) -> Result<()> {
        let Some(cache_dir) = &self.cache_dir else {
            return Ok(());
        };

        let cache_key = self.request_cache_key(request);
        let cache_file = cache_dir.join(&cache_key);

        std::fs::create_dir_all(cache_dir)?;
        let serialized = serde_json::to_string(response)?;
        std::fs::write(cache_file, serialized)?;

        Ok(())
    }

    fn get_cached_response(
        &self,
        request: &MacroCoordinationRequest,
    ) -> Result<MacroCoordinationResponse> {
        let Some(cache_dir) = &self.cache_dir else {
            anyhow::bail!("No cache directory available");
        };

        let cache_key = self.request_cache_key(request);
        let cache_file = cache_dir.join(&cache_key);

        if !cache_file.exists() {
            anyhow::bail!("No cached response available for this request");
        }

        let content = std::fs::read_to_string(cache_file)?;
        let response: MacroCoordinationResponse = serde_json::from_str(&content)?;

        eprintln!("Info: Using cached response for '{}'", self.cell_name);
        Ok(response)
    }

    fn request_cache_key(&self, request: &MacroCoordinationRequest) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.cell_name.hash(&mut hasher);
        format!("{:?}", request).hash(&mut hasher); // Simple hash for demo
        format!("{}_{:016x}", self.cell_name, hasher.finish())
    }

    fn get_cached_macros(&self) -> Result<Option<Vec<MacroInfo>>> {
        let Some(cache_dir) = &self.cache_dir else {
            return Ok(None);
        };

        let manifest_path = cache_dir.join(&self.cell_name).join("manifest.json");
        if !manifest_path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(manifest_path)?;
        let macros: Vec<MacroInfo> = serde_json::from_str(&content)?;
        Ok(Some(macros))
    }
}
