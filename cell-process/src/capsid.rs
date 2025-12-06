// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Child, Command};

pub struct Capsid;

impl Capsid {
    pub fn spawn(
        binary: &Path,
        socket_dir: &Path,
        umbilical_path: &Path,
        args: &[&str],
    ) -> Result<Child> {
        let binary_canonical = binary.canonicalize()
            .context("Binary path invalid or does not exist")?;
        
        if let Some(home) = dirs::home_dir() {
            let _trusted_root = home.join(".cell/cache/proteins");
            if !binary_canonical.is_file() {
                bail!("Target is not a file");
            }
        }

        for arg in args {
            if arg.contains(&['$', '`', ';', '|', '&', '<', '>'][..]) {
                bail!("Invalid argument characters in spawn request");
            }
        }

        let mut cmd = Command::new("bwrap");

        cmd.arg("--unshare-all")
            .arg("--share-net")
            .arg("--die-with-parent")
            // Security Hardening
            .arg("--new-session")
            .arg("--cap-drop")
            .arg("ALL")
            
            // FIX: Selective binding instead of root bind
            // We do not bind / recursively. We bind only what is necessary for a standard Linux binary.
            .arg("--ro-bind").arg("/usr").arg("/usr")
            .arg("--ro-bind").arg("/bin").arg("/bin")
            .arg("--ro-bind").arg("/sbin").arg("/sbin")
            .arg("--ro-bind").arg("/lib").arg("/lib")
            // Try to bind lib64 if it exists, otherwise ignore (bwrap fails if source missing, so this is risky without check)
            // But for a generic impl we assume FHS. If these don't exist, the host is weird.
            // Ideally we'd check existence first. For "One-Liner" robustness we assume standard distros.
            
            // Bind /etc for config/ssl/dns
            .arg("--ro-bind").arg("/etc").arg("/etc")
            
            .arg("--dev")
            .arg("/dev")
            .arg("--proc")
            .arg("/proc")
            .arg("--tmpfs")
            .arg("/tmp")
            .arg("--bind")
            .arg(socket_dir)
            .arg("/tmp/cell")
            .arg("--bind")
            .arg(umbilical_path)
            .arg("/tmp/mitosis.sock")
            .arg("--ro-bind")
            .arg(binary)
            .arg("/tmp/dna")
            .arg("/tmp/dna")
            .args(args);

        // Conditional bindings for 64-bit libs if they exist on host
        if Path::new("/lib64").exists() {
            cmd.arg("--ro-bind").arg("/lib64").arg("/lib64");
        }

        let child = cmd.spawn().context("Failed to spawn Capsid (bwrap)")?;
        Ok(child)
    }
}