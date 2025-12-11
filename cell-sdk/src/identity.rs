// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, Context, anyhow};
use cell_model::config::CellInitConfig;
use std::io::Read;
use std::sync::OnceLock;

static CONFIG: OnceLock<CellInitConfig> = OnceLock::new();

pub struct Identity;

impl Identity {
    /// Reads the configuration injected by the Root process via STDIN.
    /// This effectively "hydrates" the Cell from a generic binary into a specific node.
    /// This will block until the configuration is received.
    pub fn get() -> &'static CellInitConfig {
        CONFIG.get_or_init(|| {
            Self::bootstrap().expect("FATAL: Failed to bootstrap identity from Umbilical Cord")
        })
    }

    fn bootstrap() -> Result<CellInitConfig> {
        let mut stdin = std::io::stdin().lock();
        let mut len_buf = [0u8; 4];
        
        // 1. Read the Length Prefix (u32) - Blocks here waiting for Root
        if stdin.read_exact(&mut len_buf).is_err() {
            // If we can't read from STDIN, checking environment variables as a fallback 
            // is strictly forbidden in this architecture. We fail hard.
            return Err(anyhow!("Umbilical Cord severed: Could not read config length"));
        }
        
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        
        // 2. Read the Payload
        stdin.read_exact(&mut buf)
            .context("Failed to read config payload from STDIN")?;

        // 3. Zero-Copy Validation & Deserialization
        let archived = cell_model::rkyv::check_archived_root::<CellInitConfig>(&buf)
            .map_err(|e| anyhow!("Config corruption: {:?}", e))?;

        let config: CellInitConfig = archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap();
        
        Ok(config)
    }
}