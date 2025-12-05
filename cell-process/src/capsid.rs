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
            .arg("--ro-bind")
            .arg("/")
            .arg("/")
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

        let child = cmd.spawn().context("Failed to spawn Capsid (bwrap)")?;
        Ok(child)
    }
}