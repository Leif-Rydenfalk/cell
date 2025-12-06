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

        // SECURITY: Strict Sandboxing Profile
        cmd.arg("--unshare-all")
            .arg("--share-net")
            .arg("--die-with-parent")
            .arg("--new-session")
            .arg("--cap-drop").arg("ALL")
            
            // 1. Filesystem: Whitelist, don't Blacklist.
            // Do NOT bind-mount / (root).
            
            // 2. Base System (Read-Only)
            .arg("--ro-bind").arg("/usr").arg("/usr")
            .arg("--ro-bind").arg("/bin").arg("/bin")
            .arg("--ro-bind").arg("/sbin").arg("/sbin");

        // 3. Libraries (Read-Only)
        // Check for existence to prevent bwrap failure on different distros
        if Path::new("/lib").exists() {
            cmd.arg("--ro-bind").arg("/lib").arg("/lib");
        }
        if Path::new("/lib64").exists() {
            cmd.arg("--ro-bind").arg("/lib64").arg("/lib64");
        }

        // 4. Configuration (Selective)
        if Path::new("/etc/resolv.conf").exists() {
            cmd.arg("--ro-bind").arg("/etc/resolv.conf").arg("/etc/resolv.conf");
        }
        if Path::new("/etc/hosts").exists() {
            cmd.arg("--ro-bind").arg("/etc/hosts").arg("/etc/hosts");
        }
        if Path::new("/etc/ssl/certs").exists() {
            cmd.arg("--ro-bind").arg("/etc/ssl/certs").arg("/etc/ssl/certs");
        }
        if Path::new("/etc/ca-certificates").exists() {
            cmd.arg("--ro-bind").arg("/etc/ca-certificates").arg("/etc/ca-certificates");
        }

        // 5. Devices & Runtime (Fresh Instances)
        cmd.arg("--dev").arg("/dev")
           .arg("--proc").arg("/proc")
           .arg("--tmpfs").arg("/tmp")
           
           // 6. Cell runtime requirements
           .arg("--bind").arg(socket_dir).arg("/tmp/cell")
           .arg("--bind").arg(umbilical_path).arg("/tmp/mitosis.sock")
           
           // 7. The Payload
           .arg("--ro-bind").arg(binary_canonical).arg("/tmp/dna/payload")
           .args(args);

        let child = cmd.spawn().context("Failed to spawn Capsid (bwrap)")?;
        Ok(child)
    }
}