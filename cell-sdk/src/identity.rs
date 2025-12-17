// SPDX-License-Identifier: MIT
// cell-sdk/src/identity.rs

use cell_model::config::CellInitConfig;
use std::sync::OnceLock;

static CONFIG: OnceLock<CellInitConfig> = OnceLock::new();

pub struct Identity;

impl Identity {
    pub fn get() -> &'static CellInitConfig {
        CONFIG.get_or_init(|| Self::bootstrap())
    }

    fn bootstrap() -> CellInitConfig {
        // 1. Try Environment Variable (Production/Docker way)
        let node_id = std::env::var("CELL_NODE_ID")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(|| {
                // 2. Fallback to Random (Dev/Test way)
                rand::random()
            });

        // 3. Determine Name from CWD
        let cell_name = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "unknown".to_string());

        CellInitConfig {
            node_id,
            cell_name,
            peers: vec![],
            socket_path: String::new(), // Deprecated in FS topology
            organism: std::env::var("CELL_ORGANISM").unwrap_or_else(|_| "default".to_string()),
        }
    }
}
