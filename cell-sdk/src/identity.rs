// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Result, anyhow};
use cell_model::config::CellInitConfig;
use cell_model::protocol::{MitosisSignal, MitosisControl};
use cell_transport::GapJunction;
use std::sync::{OnceLock, Mutex};

// We store the junction temporarily here so Runtime can access it later for signaling.
pub static GAP_JUNCTION: OnceLock<Mutex<GapJunction>> = OnceLock::new();
static CONFIG: OnceLock<CellInitConfig> = OnceLock::new();

pub struct Identity;

impl Identity {
    /// Hydrates the Cell via the Gap Junction (FD 3).
    /// Sends `RequestIdentity` and waits for `InjectIdentity`.
    /// This blocks the process boot.
    pub fn get() -> &'static CellInitConfig {
        CONFIG.get_or_init(|| {
            Self::bootstrap().expect("FATAL: Failed to bootstrap identity via Gap Junction")
        })
    }

    fn bootstrap() -> Result<CellInitConfig> {
        // 1. Open the physical channel (FD 3)
        // UNSAFE: We assume the Hypervisor respected the protocol.
        // We do NOT check STDIN anymore.
        let mut junction = unsafe { GapJunction::open_daughter()? };

        // 2. Signal: Request Identity
        junction.signal(MitosisSignal::RequestIdentity)?;

        // 3. Wait: Receive Control
        let control = junction.wait_for_control()?;

        match control {
            MitosisControl::InjectIdentity(config) => {
                // Save the junction for later Lifecycle signaling (Prometaphase/Cytokinesis)
                let _ = GAP_JUNCTION.set(Mutex::new(junction));
                Ok(config)
            }
            MitosisControl::Terminate => {
                Err(anyhow!("Hypervisor aborted mitosis"))
            }
        }
    }
}