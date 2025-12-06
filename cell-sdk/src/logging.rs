// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use tracing::{info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init_logging(cell_name: &str) {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_level(true)
                .with_thread_ids(true)
                .json()
        )
        .init();

    info!(
        cell_name = cell_name,
        version = env!("CARGO_PKG_VERSION"),
        "Cell initialized"
    );
}

// Add to every critical operation:
#[macro_export]
macro_rules! log_operation {
    ($op:expr, $result:expr) => {
        match $result {
            Ok(val) => {
                tracing::info!(operation = $op, status = "success");
                Ok(val)
            }
            Err(e) => {
                tracing::error!(operation = $op, error = %e, status = "failed");
                Err(e)
            }
        }
    };
}