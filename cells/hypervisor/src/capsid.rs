// cells/hypervisor/src/capsid.rs
// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use cell_model::config::CellInitConfig;
use cell_model::protocol::{MitosisSignal, MitosisControl};
use cell_transport::gap_junction::{spawn_with_gap_junction, GapJunction};
use tracing::{info, warn};

pub struct Capsid;

impl Capsid {
    pub fn spawn(
        binary: &Path,
        socket_dir: &Path,
        daemon_socket_path: &Path,
        args: &[&str],
        config: &CellInitConfig,
        capture_output: bool, // New flag
    ) -> Result<Child> {
        let binary_canonical = binary.canonicalize()
            .context("Binary path invalid or does not exist")?;
        
        // ... (bwrap check omitted for brevity, assuming standard logic) ...
        // Re-implementing simplified logic to apply capture_output

        let has_bwrap = which::which("bwrap").is_ok();
        let mut cmd = if has_bwrap {
            let mut c = Command::new("bwrap");
            c.arg("--unshare-all")
             .arg("--share-net")
             .arg("--die-with-parent")
             .arg("--new-session")
             .arg("--cap-drop").arg("ALL")
             .arg("--ro-bind").arg("/usr").arg("/usr")
             .arg("--ro-bind").arg("/bin").arg("/bin")
             .arg("--ro-bind").arg("/sbin").arg("/sbin")
             .arg("--dev").arg("/dev")
             .arg("--proc").arg("/proc")
             .arg("--tmpfs").arg("/tmp")
             .arg("--bind").arg(socket_dir).arg("/tmp/cell")
             .arg("--bind").arg(daemon_socket_path).arg("/tmp/mitosis.sock")
             .arg("--ro-bind").arg(&binary_canonical).arg("/tmp/dna/payload");

            if Path::new("/lib").exists() { c.arg("--ro-bind").arg("/lib").arg("/lib"); }
            if Path::new("/lib64").exists() { c.arg("--ro-bind").arg("/lib64").arg("/lib64"); }

            c.args(args);
            c.arg("/tmp/dna/payload");
            c.env("CELL_SOCKET_DIR", "/tmp/cell");
            c
        } else {
            let mut c = Command::new(&binary_canonical);
            c.args(args);
            c.env("CELL_SOCKET_DIR", socket_dir);
            c
        };

        cmd.env("CELL_ORGANISM", &config.organism);
        cmd.env_remove("CELL_NODE_ID"); 
        cmd.env_remove("CELL_IDENTITY");

        // IO Configuration
        cmd.stdin(Stdio::null());
        if capture_output {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        } else {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::inherit());
        }

        let (child, mut junction) = spawn_with_gap_junction(cmd)
            .context("Failed to spawn Capsid")?;

        let config_clone = config.clone();
        
        std::thread::spawn(move || {
            loop {
                match junction.wait_for_signal() {
                    Ok(signal) => {
                        match signal {
                            MitosisSignal::RequestIdentity => {
                                let _ = junction.send_control(MitosisControl::InjectIdentity(config_clone.clone()));
                            }
                            MitosisSignal::Cytokinesis => break,
                            MitosisSignal::Apoptosis { reason } => {
                                tracing::error!("Cell Apoptosis: {}", reason);
                                break;
                            }
                            MitosisSignal::Necrosis => {
                                tracing::error!("Cell Necrosis.");
                                break;
                            }
                            _ => {}
                        }
                    }
                    Err(_) => break, 
                }
            }
        });

        Ok(child)
    }
}