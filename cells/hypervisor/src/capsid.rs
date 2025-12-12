// cells/hypervisor/src/capsid.rs
// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use cell_model::config::CellInitConfig;
use cell_model::protocol::{MitosisSignal, MitosisControl};
use cell_transport::gap_junction::{spawn_with_gap_junction, GapJunction};

pub struct Capsid;

impl Capsid {
    pub fn spawn(
        binary: &Path,
        socket_dir: &Path,
        daemon_socket_path: &Path, // Renamed from umbilical_path
        args: &[&str],
        config: &CellInitConfig,
    ) -> Result<Child> {
        let binary_canonical = binary.canonicalize()
            .context("Binary path invalid or does not exist")?;
        
        if let Some(home) = dirs::home_dir() {
            let _trusted_root = home.join(".cell/bin");
            if !binary_canonical.is_file() {
                bail!("Target is not a file");
            }
        }

        let mut cmd = Command::new("bwrap");

        // SECURITY: Strict Sandboxing Profile
        cmd.arg("--unshare-all")
            .arg("--share-net")
            .arg("--die-with-parent")
            .arg("--new-session")
            .arg("--cap-drop").arg("ALL")
            
            // 1. Filesystem: Whitelist
            .arg("--ro-bind").arg("/usr").arg("/usr")
            .arg("--ro-bind").arg("/bin").arg("/bin")
            .arg("--ro-bind").arg("/sbin").arg("/sbin")
            .arg("--dev").arg("/dev")
            .arg("--proc").arg("/proc")
            .arg("--tmpfs").arg("/tmp")
            
            // 2. Cell runtime requirements
            .arg("--bind").arg(socket_dir).arg("/tmp/cell")
            // Map the daemon socket (mitosis.sock) to a known location if needed, 
            // though the Cell should find it via standard paths or just use Gap Junction for boot.
            // We keep the bind but rename the internal target to be clear.
            .arg("--bind").arg(daemon_socket_path).arg("/tmp/mitosis.sock")
            
            // 3. The Payload
            .arg("--ro-bind").arg(binary_canonical).arg("/tmp/dna/payload");

        // 4. Bind optional libs
        if Path::new("/lib").exists() { cmd.arg("--ro-bind").arg("/lib").arg("/lib"); }
        if Path::new("/lib64").exists() { cmd.arg("--ro-bind").arg("/lib64").arg("/lib64"); }

        // 5. Clean IO
        cmd.stdin(Stdio::null()); // Explicitly Null. No Umbilical.
        cmd.stdout(Stdio::null()); // Using Gap Junction on FD 3
        cmd.stderr(Stdio::inherit()); 

        // 6. Config
        cmd.env("CELL_SOCKET_DIR", "/tmp/cell");
        // REMOVED: cmd.env("CELL_UMBILICAL", ...); 
        // The cell finds the daemon socket via standard paths or we don't need it for boot.
        // If the cell needs to talk to the daemon later, it uses resolve_socket_dir joined with "mitosis.sock".
        // In the container, that is mapped to /tmp/mitosis.sock.
        // We can optionally set CELL_DAEMON_SOCKET env if we want to be explicit, but the standard path logic covers it.
        
        cmd.env("CELL_ORGANISM", &config.organism);
        cmd.env_remove("CELL_NODE_ID"); 
        cmd.env_remove("CELL_IDENTITY");

        cmd.args(args);
        cmd.arg("/tmp/dna/payload");

        // --- SPAWN WITH GAP JUNCTION ---
        // This maps a socketpair to FD 3 in the child
        let (child, mut junction) = spawn_with_gap_junction(cmd)
            .context("Failed to spawn Capsid with Gap Junction")?;

        // --- THE MITOSIS HANDSHAKE ---
        let config_clone = config.clone();
        
        std::thread::spawn(move || {
            // Monitor the birth
            loop {
                match junction.wait_for_signal() {
                    Ok(signal) => {
                        match signal {
                            MitosisSignal::RequestIdentity => {
                                let _ = junction.send_control(MitosisControl::InjectIdentity(config_clone.clone()));
                            }
                            MitosisSignal::Cytokinesis => {
                                // Cell is independent. Close junction.
                                break;
                            }
                            MitosisSignal::Apoptosis { reason } => {
                                tracing::error!("Cell Apoptosis: {}", reason);
                                break;
                            }
                            MitosisSignal::Necrosis => {
                                tracing::error!("Cell Necrosis.");
                                break;
                            }
                            _ => {} // Ignore status updates
                        }
                    }
                    Err(_) => break, // Connection lost
                }
            }
        });

        Ok(child)
    }
}