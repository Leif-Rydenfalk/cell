// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use std::os::unix::io::{FromRawFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::io::{Read, Write};
use anyhow::{Result, anyhow, Context};
use cell_model::protocol::{MitosisSignal, MitosisControl, GAP_JUNCTION_FD};
use rkyv::Deserialize;

/// A synchronous, biological link between Progenitor and Daughter.
/// Used exclusively during the boot phase (Mitosis) before the Async Runtime takes over.
pub struct GapJunction {
    stream: UnixStream,
}

impl GapJunction {
    /// Open the Gap Junction from the Daughter side (FD 3).
    /// This is unsafe because it assumes FD 3 is valid and owned by us.
    /// It should be called exactly once at startup.
    pub unsafe fn open_daughter() -> Result<Self> {
        // validate FD 3 exists? 
        // We assume the Hypervisor set it up correctly via dup2.
        let stream = UnixStream::from_raw_fd(GAP_JUNCTION_FD);
        // We clone it so the original FD 3 technically stays open if we drop this, 
        // but for ownership semantics in Rust we usually want to own it.
        // `from_raw_fd` takes ownership.
        Ok(Self { stream })
    }

    /// Wrap an existing stream (Progenitor side)
    pub fn from_stream(stream: UnixStream) -> Self {
        Self { stream }
    }

    // --- Daughter Methods ---

    pub fn signal(&mut self, signal: MitosisSignal) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 256>(&signal)?.into_vec();
        let len = (bytes.len() as u32).to_le_bytes();
        self.stream.write_all(&len)?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()?;
        Ok(())
    }

    pub fn wait_for_control(&mut self) -> Result<MitosisControl> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).context("Gap Junction severed by Progenitor")?;
        let len = u32::from_le_bytes(len_buf) as usize;
        
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).context("Gap Junction severed during payload")?;

        let archived = cell_model::rkyv::check_archived_root::<MitosisControl>(&buf)
            .map_err(|e| anyhow!("Invalid control signal: {:?}", e))?;
        
        Ok(archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap())
    }

    // --- Progenitor Methods ---

    pub fn send_control(&mut self, control: MitosisControl) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 1024>(&control)?.into_vec();
        let len = (bytes.len() as u32).to_le_bytes();
        self.stream.write_all(&len)?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()?;
        Ok(())
    }

    pub fn wait_for_signal(&mut self) -> Result<MitosisSignal> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).context("Gap Junction severed by Daughter")?;
        let len = u32::from_le_bytes(len_buf) as usize;
        
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).context("Gap Junction severed during payload")?;

        let archived = cell_model::rkyv::check_archived_root::<MitosisSignal>(&buf)
            .map_err(|e| anyhow!("Invalid signal from Daughter: {:?}", e))?;
        
        Ok(archived.deserialize(&mut cell_model::rkyv::Infallible).unwrap())
    }
}

/// Helper to create the bridge and map FDs for a child process.
/// This uses `nix` to perform the `dup2` dance safely.
#[cfg(feature = "shm")]
pub fn spawn_with_gap_junction(mut cmd: std::process::Command) -> Result<(std::process::Child, GapJunction)> {
    use std::os::unix::process::CommandExt;
    
    let (parent_sock, child_sock) = UnixStream::pair()?;
    
    // We need to keep child_sock alive until the exec happens, but not in the parent after spawn.
    // The `pre_exec` closure runs in the child context.
    
    // We can't move `child_sock` into the Fn closure easily because CommandExt::pre_exec takes FnMut
    // and `child_sock` is not Copy. However, we can use the RawFd.
    let child_fd = child_sock.as_raw_fd();

    unsafe {
        cmd.pre_exec(move || {
            // This runs in the child process after fork, before exec.
            // 1. Dup the socket to FD 3
            if nix::unistd::dup2(child_fd, GAP_JUNCTION_FD).is_err() {
                return Err(std::io::Error::last_os_error());
            }
            
            // 2. Clear CLOEXEC on FD 3 so it survives into the Cell binary
            // (dup2 creates a new FD, we must ensure it doesn't close)
            // By default dup2'd FDs do NOT have FD_CLOEXEC set, so we are good.
            // But we should verify the source `child_sock` isn't closing it.
            // Actually, we should probably close `child_fd` (the original one) to avoid leaks,
            // but `dup2` handles the target.
            
            Ok(())
        });
    }

    let child = cmd.spawn()?;
    
    // Parent closes its handle to the child's end
    drop(child_sock);

    let junction = GapJunction::from_stream(parent_sock);
    Ok((child, junction))
}