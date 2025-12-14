// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{anyhow, Result};
use cell_model::config::CellInitConfig;
use cell_model::protocol::{MitosisControl, MitosisSignal};
use cell_transport::GapJunction;
use std::sync::{Mutex, OnceLock};

pub static GAP_JUNCTION: OnceLock<Mutex<GapJunction>> = OnceLock::new();
static CONFIG: OnceLock<CellInitConfig> = OnceLock::new();

pub struct Identity;

impl Identity {
    pub fn get() -> &'static CellInitConfig {
        CONFIG.get_or_init(|| {
            Self::bootstrap().expect("FATAL: Failed to bootstrap identity via Gap Junction")
        })
    }

    fn bootstrap() -> Result<CellInitConfig> {
        let mut junction = unsafe { GapJunction::open_daughter()? };

        junction.signal(MitosisSignal::RequestIdentity)?;

        let control = junction.wait_for_control()?;

        match control {
            MitosisControl::InjectIdentity(config) => {
                let _ = GAP_JUNCTION.set(Mutex::new(junction));
                Ok(config)
            }
            MitosisControl::Terminate => Err(anyhow!("Hypervisor aborted mitosis")),
        }
    }
}
