// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use anyhow::Result;
use cell_model::protocol::{MitosisControl, MitosisSignal};
use cell_model::rkyv::Deserialize;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};

#[cfg(feature = "std")]
use nix::libc;

pub struct GapJunction {
    pipe: File,
}

impl GapJunction {
    /// Open the file descriptor 3 inherited from the parent (The Umbilical Cord)
    pub unsafe fn open_daughter() -> Result<Self> {
        let file = File::from_raw_fd(3);
        Ok(Self { pipe: file })
    }

    /// For the parent (Hypervisor) to wrap a socket end
    pub fn from_file(file: File) -> Self {
        Self { pipe: file }
    }

    pub fn signal(&mut self, signal: MitosisSignal) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 1024>(&signal)?.into_vec();
        let len = bytes.len() as u32;
        self.pipe.write_all(&len.to_le_bytes())?;
        self.pipe.write_all(&bytes)?;
        Ok(())
    }

    pub fn wait_for_signal(&mut self) -> Result<MitosisSignal> {
        let mut len_buf = [0u8; 4];
        self.pipe.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.pipe.read_exact(&mut buf)?;

        let archived = cell_model::rkyv::check_archived_root::<MitosisSignal>(&buf)
            .map_err(|_| anyhow::anyhow!("Invalid signal"))?;
        Ok(archived
            .deserialize(&mut cell_model::rkyv::Infallible)
            .unwrap())
    }

    pub fn send_control(&mut self, control: MitosisControl) -> Result<()> {
        let bytes = cell_model::rkyv::to_bytes::<_, 1024>(&control)?.into_vec();
        let len = bytes.len() as u32;
        self.pipe.write_all(&len.to_le_bytes())?;
        self.pipe.write_all(&bytes)?;
        Ok(())
    }

    pub fn wait_for_control(&mut self) -> Result<MitosisControl> {
        let mut len_buf = [0u8; 4];
        self.pipe.read_exact(&mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.pipe.read_exact(&mut buf)?;

        let archived = cell_model::rkyv::check_archived_root::<MitosisControl>(&buf)
            .map_err(|_| anyhow::anyhow!("Invalid control"))?;
        Ok(archived
            .deserialize(&mut cell_model::rkyv::Infallible)
            .unwrap())
    }
}

pub fn spawn_with_gap_junction(
    mut cmd: std::process::Command,
) -> Result<(std::process::Child, GapJunction)> {
    use std::os::unix::process::CommandExt;

    // Create a bidirectional socket pair (parent <-> child)
    let (parent_sock, child_sock) = std::os::unix::net::UnixStream::pair()?;
    let child_fd = child_sock.as_raw_fd();

    // We rely on CommandExt::pre_exec to map the socket to FD 3 in the child process
    unsafe {
        cmd.pre_exec(move || {
            // If the socket isn't already FD 3, dup2 it there
            if child_fd != 3 {
                // Ignore errors here; if it fails, the child will likely crash on startup anyway
                let _ = libc::dup2(child_fd, 3);
            }

            // Important: Clear the CLOEXEC flag on FD 3 so it stays open across exec
            let flags = libc::fcntl(3, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }

            Ok(())
        });
    }

    // Spawn the child
    let child = cmd.spawn()?;

    // The child has its copy (as FD 3). We keep 'parent_sock' wrapped in GapJunction.
    // 'child_sock' is dropped here, closing the parent's handle to that end.
    // Explicit conversion from UnixStream to File via RawFd
    let junction = GapJunction::from_file(unsafe { File::from_raw_fd(parent_sock.into_raw_fd()) });

    Ok((child, junction))
}
