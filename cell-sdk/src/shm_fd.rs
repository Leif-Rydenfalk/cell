// SPDX-License-Identifier: MIT
// cell-sdk/src/shm_fd.rs
//! File descriptor passing over Unix sockets for GPU memory sharing

use anyhow::{Context, Result};
use nix::sys::socket::{recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags};
use std::io::{IoSlice, IoSliceMut};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream as StdUnixStream;
use tokio::net::UnixStream;

/// Send a file descriptor over a Unix socket
pub async fn send_fd(stream: &mut UnixStream, fd: RawFd) -> Result<()> {
    let std_stream = stream.as_raw_fd();
    let iov = [IoSlice::new(&[0u8; 1])]; // Dummy byte to trigger CMSG
    let cmsg = [ControlMessage::ScmRights(&[fd])];

    tokio::task::spawn_blocking(move || {
        sendmsg::<()>(std_stream, &iov, &cmsg, MsgFlags::empty(), None)
            .context("Failed to send file descriptor")?;
        Ok::<_, anyhow::Error>(())
    })
    .await??;

    Ok(())
}

/// Receive a file descriptor from a Unix socket
pub async fn recv_fd(stream: &mut UnixStream) -> Result<RawFd> {
    let std_stream = stream.as_raw_fd();
    let mut buf = [0u8; 1];
    let mut iov = [IoSliceMut::new(&mut buf)];
    let mut cmsg_buf = nix::cmsg_space!([RawFd; 1]);

    let received_fds = tokio::task::spawn_blocking(move || {
        let msg = recvmsg::<()>(std_stream, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())
            .context("Failed to receive file descriptor")?;

        let mut fds = Vec::new();
        for cmsg in msg.cmsgs() {
            if let ControlMessageOwned::ScmRights(r) = cmsg {
                fds.extend_from_slice(&r);
            }
        }
        Ok::<_, anyhow::Error>(fds)
    })
    .await??;

    received_fds
        .first()
        .copied()
        .context("No file descriptor received")
}

/// Create a dma-buf file descriptor for GPU shared memory
#[cfg(target_os = "linux")]
pub fn create_dma_buf(size: usize) -> Result<RawFd> {
    use nix::fcntl::{fcntl, FcntlArg, F_ADD_SEALS, F_GET_SEALS, F_SEAL_SEAL};
    use std::fs::File;
    use std::os::unix::fs::OpenOptionsExt;

    // Create memfd with dma-buf compatibility
    let fd = nix::sys::memfd::memfd_create(
        &std::ffi::CString::new("cell-gpu-buffer")?,
        nix::sys::memfd::MemFdCreateFlag::MFD_ALLOW_SEALING
            | nix::sys::memfd::MemFdCreateFlag::MFD_CLOEXEC,
    )?;

    // Set size
    nix::unistd::ftruncate(fd, size as i64)?;

    // Add seals for read-only sharing
    fcntl(
        fd,
        FcntlArg::F_ADD_SEALS(nix::fcntl::SealFlag::F_SEAL_SHRINK),
    )?;
    fcntl(fd, FcntlArg::F_ADD_SEALS(nix::fcntl::SealFlag::F_SEAL_GROW))?;
    fcntl(
        fd,
        FcntlArg::F_ADD_SEALS(nix::fcntl::SealFlag::F_SEAL_WRITE),
    )?;
    fcntl(fd, FcntlArg::F_ADD_SEALS(nix::fcntl::SealFlag::F_SEAL_SEAL))?;

    Ok(fd)
}

#[cfg(not(target_os = "linux"))]
pub fn create_dma_buf(size: usize) -> Result<RawFd> {
    anyhow::bail!("dma-buf only supported on Linux")
}
