// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use std::path::Path;
use std::process::Command;

pub enum Runtime {
    Raw,    // For Dev: Just spawn the binary
    Podman, // For Prod: Rootless Containers
}

pub fn build_spawn_cmd(
    runtime: Runtime,
    binary_name: &str,
    dna_path: &Path,
    socket_dir: &Path,
    umbilical_path: &Path,
) -> Result<Command> {
    match runtime {
        Runtime::Raw => {
            let mut cmd = Command::new(dna_path.join(binary_name));
            // In Raw mode, we just pass env vars to point to sockets
            cmd.env("CELL_SOCKET_DIR", socket_dir);
            cmd.env("CELL_UMBILICAL", umbilical_path);
            Ok(cmd)
        }
        Runtime::Podman => {
            let uid = users::get_current_uid();
            let gid = users::get_current_gid();

            let mut cmd = Command::new("podman");
            cmd.arg("run")
                .arg("--rm")
                .arg("--detach")
                .arg("--network")
                .arg("none") // Security: No Network
                .arg("--read-only") // Security: Immutable FS
                // Mount DNA (Binaries) as Read-Only
                .arg("-v")
                .arg(format!("{}:/dna:ro", dna_path.display()))
                // Mount Sockets (Shared Memory / Comm) as Read-Write
                .arg("-v")
                .arg(format!("{}:/tmp/cell", socket_dir.display()))
                // Mount The Umbilical Cord (Recursive Spawning capability)
                .arg("-v")
                .arg(format!("{}:/mitosis.sock", umbilical_path.display()))
                // Identity Map (So created socket files are owned by us)
                .arg("--user")
                .arg(format!("{}:{}", uid, gid))
                // Resource Limits
                .arg("--cpus")
                .arg("1.0")
                .arg("--memory")
                .arg("512m")
                // Image
                .arg("alpine:latest")
                // The Command
                .arg(format!("/dna/{}", binary_name));

            Ok(cmd)
        }
    }
}