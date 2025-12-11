// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::io::Write;
use cell_model::config::CellInitConfig;

pub struct Capsid;

impl Capsid {
    pub fn spawn(
        binary: &Path,
        socket_dir: &Path,
        umbilical_path: &Path,
        args: &[&str],
        config: &CellInitConfig,
    ) -> Result<Child> {
        let binary_canonical = binary.canonicalize()
            .context("Binary path invalid or does not exist")?;
        
        if let Some(home) = dirs::home_dir() {
            let _trusted_root = home.join(".cell/cache/proteins");
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
            
            // 1. Filesystem: Whitelist, don't Blacklist.
            .arg("--ro-bind").arg("/usr").arg("/usr")
            .arg("--ro-bind").arg("/bin").arg("/bin")
            .arg("--ro-bind").arg("/sbin").arg("/sbin")
            .arg("--dev").arg("/dev")
            .arg("--proc").arg("/proc")
            .arg("--tmpfs").arg("/tmp")
            
            // 2. Cell runtime requirements
            .arg("--bind").arg(socket_dir).arg("/tmp/cell")
            .arg("--bind").arg(umbilical_path).arg("/tmp/mitosis.sock")
            
            // 3. The Payload
            .arg("--ro-bind").arg(binary_canonical).arg("/tmp/dna/payload");

        // 4. Bind optional libs
        if Path::new("/lib").exists() { cmd.arg("--ro-bind").arg("/lib").arg("/lib"); }
        if Path::new("/lib64").exists() { cmd.arg("--ro-bind").arg("/lib64").arg("/lib64"); }

        // 5. Inject Identity via STDIN (Umbilical Cord)
        cmd.stdin(Stdio::piped());

        // 6. Config - Only strictly necessary ENV vars allowed (path locations)
        cmd.env("CELL_SOCKET_DIR", "/tmp/cell");
        cmd.env("CELL_UMBILICAL", "/tmp/mitosis.sock");
        // NOTE: CELL_NODE_ID and others are purposely REMOVED.
        cmd.env_remove("CELL_NODE_ID");
        cmd.env_remove("CELL_IDENTITY");

        cmd.args(args);
        // The binary is generic; it doesn't even know its name until we inject it.
        cmd.arg("/tmp/dna/payload");

        let mut child = cmd.spawn().context("Failed to spawn Capsid (bwrap)")?;

        // --- THE INJECTION ---
        if let Some(mut stdin) = child.stdin.take() {
            // Serialize Config to Bytes (Zero-Copy compatible format)
            let bytes = cell_model::rkyv::to_bytes::<_, 1024>(config)
                .context("Failed to serialize init config")?
                .into_vec();
            
            // Wire Protocol: [Length u32][Data...]
            let len = (bytes.len() as u32).to_le_bytes();
            
            stdin.write_all(&len).context("Failed to inject config length")?;
            stdin.write_all(&bytes).context("Failed to inject config payload")?;
            stdin.flush().context("Failed to flush umbilical cord")?;
            
            // Dropping stdin closes the pipe, signaling EOF to the child.
        }

        Ok(child)
    }
}