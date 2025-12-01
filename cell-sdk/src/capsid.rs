// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result};
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
        let mut cmd = Command::new("bwrap");

        cmd.arg("--unshare-all")
            .arg("--share-net") // Keep net for now (easier debugging/downloading if needed)
            .arg("--die-with-parent")
            // 1. Root Filesystem Setup
            .arg("--ro-bind")
            .arg("/")
            .arg("/")
            .arg("--dev")
            .arg("/dev")
            .arg("--proc")
            .arg("/proc")
            // 2. Mutable Scratch Space
            // We mount a tmpfs on /tmp. This allows us to create mount points inside /tmp
            // even though / is read-only.
            .arg("--tmpfs")
            .arg("/tmp")
            // 3. Mounts
            // Host: ~/.cell/run -> Container: /tmp/cell
            .arg("--bind")
            .arg(socket_dir)
            .arg("/tmp/cell")
            // Host: ~/.cell/run/mitosis.sock -> Container: /tmp/mitosis.sock
            .arg("--bind")
            .arg(umbilical_path)
            .arg("/tmp/mitosis.sock")
            // 4. DNA (The Binary)
            // We mount the binary into /tmp because creating /app on a read-only root fails
            // if /app doesn't exist on the host.
            .arg("--ro-bind")
            .arg(binary)
            .arg("/tmp/dna")
            // 5. Exec
            .arg("/tmp/dna")
            .args(args);

        let child = cmd.spawn().context("Failed to spawn Capsid (bwrap)")?;
        Ok(child)
    }
}
