// SPDX-License-Identifier: MIT
// cell-sdk/src/io_client.rs: The Pragmatic Beggar
// Tries to offload IO to a router, but isn't afraid to get its hands dirty
// if no router is found.

use anyhow::{Context, Result};
use cell_model::io::{IoRequest, IoResponse};
use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
use rkyv::Deserialize;
use std::io::IoSliceMut;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tracing::info;
use tracing::warn;

pub struct IoClient;

impl IoClient {
    /// Connects to the IO cell and requests a bound listener FD.
    /// FALLBACK: If IO cell is down, binds locally.
    pub async fn bind_membrane(cell_name: &str) -> Result<std::os::unix::net::UnixListener> {
        // 1. Try Router
        if let Ok(mut stream) = Self::connect_to_io().await {
            let req = IoRequest::Bind {
                cell_name: cell_name.to_string(),
            };
            if let Ok((resp, fds)) = Self::rpc(&mut stream, req).await {
                if !matches!(resp, IoResponse::Error { .. }) {
                    if let Some(fd) = fds.first() {
                        let listener =
                            unsafe { std::os::unix::net::UnixListener::from_raw_fd(*fd) };
                        return Ok(listener);
                    }
                }
            }
        }

        // 2. Fallback: Local Bind
        warn!("[IO] Router unavailable. Falling back to local syscalls.");

        let cwd = std::env::current_dir().context("No CWD")?;
        let io_dir = cwd.join(".cell/io");
        std::fs::create_dir_all(&io_dir)?;

        // Bind to CWD/.cell/io/in (the membrane socket)
        let sock_path = io_dir.join("in");
        if sock_path.exists() {
            std::fs::remove_file(&sock_path)?;
        }

        let listener = std::os::unix::net::UnixListener::bind(&sock_path)?;

        // ALSO create the bootstrap socket for global discovery
        let home = dirs::home_dir().context("No HOME dir")?;
        let global_io = home.join(".cell/io");
        std::fs::create_dir_all(&global_io)?;
        let global_sock = global_io.join(format!("{}.sock", cell_name));
        if global_sock.exists() {
            std::fs::remove_file(&global_sock)?;
        }

        // Create symlink from global to local for compatibility
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&sock_path, &global_sock).ok();
        }

        // CRITICAL: Create the neighbor structure so other cells can find us
        // When running in a workspace, other cells look in .cell/neighbors/
        let neighbors_dir = cwd.join(".cell/neighbors");
        std::fs::create_dir_all(&neighbors_dir)?;
        let cell_neighbor_dir = neighbors_dir.join(cell_name);
        std::fs::create_dir_all(&cell_neighbor_dir)?;

        let tx_link = cell_neighbor_dir.join("tx");
        if tx_link.exists() || std::fs::symlink_metadata(&tx_link).is_ok() {
            std::fs::remove_file(&tx_link).ok();
        }

        // Point the neighbor link to our socket
        #[cfg(unix)]
        std::os::unix::fs::symlink(&sock_path, &tx_link)
            .context("Failed to create neighbor tx symlink")?;

        info!(
            "[IO] Bound membrane at {:?}, neighbor link at {:?}",
            sock_path, tx_link
        );

        Ok(listener)
    }

    /// Connects to the IO cell and requests a connection to a target.
    /// FALLBACK: If IO cell is down, connects directly to target socket.
    ///
    /// RETRY: Implements exponential backoff for connection attempts
    pub async fn connect(target: &str) -> Result<std::os::unix::net::UnixStream> {
        // Retry configuration
        const MAX_RETRIES: u32 = 10;
        const INITIAL_DELAY_MS: u64 = 100;
        const MAX_DELAY_MS: u64 = 2000;

        let mut delay_ms = INITIAL_DELAY_MS;

        for attempt in 0..MAX_RETRIES {
            // Try to connect via multiple methods
            match Self::try_connect_once(target).await {
                Ok(stream) => {
                    if attempt > 0 {
                        tracing::info!(
                            "[IO] Connected to '{}' after {} attempts",
                            target,
                            attempt + 1
                        );
                    }
                    return Ok(stream);
                }
                Err(e) => {
                    if attempt == 0 {
                        tracing::debug!("[IO] Initial connection to '{}' failed: {}", target, e);
                    }

                    if attempt < MAX_RETRIES - 1 {
                        tracing::debug!(
                            "[IO] Retrying '{}' in {}ms (attempt {}/{})",
                            target,
                            delay_ms,
                            attempt + 1,
                            MAX_RETRIES
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

                        // Exponential backoff with cap
                        delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
                    } else {
                        return Err(e).with_context(|| {
                            format!(
                                "Failed to connect to '{}' after {} attempts",
                                target, MAX_RETRIES
                            )
                        });
                    }
                }
            }
        }

        unreachable!()
    }

    async fn try_connect_once(target: &str) -> Result<std::os::unix::net::UnixStream> {
        // 1. Try Router (IO Cell)
        if let Ok(mut stream) = Self::connect_to_io().await {
            let req = IoRequest::Connect {
                target_cell: target.to_string(),
            };
            if let Ok((resp, fds)) = Self::rpc(&mut stream, req).await {
                if !matches!(resp, IoResponse::Error { .. }) {
                    if let Some(fd) = fds.first() {
                        let socket = unsafe { std::os::unix::net::UnixStream::from_raw_fd(*fd) };
                        return Ok(socket);
                    }
                }
            }
        }

        // 2. Check neighbor link FIRST (before global registry)
        let cwd = std::env::current_dir()?;
        let neighbor_link = cwd.join(".cell/neighbors").join(target).join("tx");

        if neighbor_link.exists() {
            // Check if the symlink target exists
            match std::fs::read_link(&neighbor_link) {
                Ok(target_path) => {
                    if !target_path.exists() {
                        // Symlink exists but target doesn't - cell not ready yet
                        return Err(anyhow::anyhow!(
                            "Neighbor link exists but target socket not ready"
                        ));
                    }
                }
                Err(_) => {
                    // Can't read symlink, try anyway
                }
            }

            match std::os::unix::net::UnixStream::connect(&neighbor_link) {
                Ok(socket) => return Ok(socket),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(anyhow::anyhow!(
                        "Neighbor socket not found (cell not ready)"
                    ));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Failed to connect to neighbor: {}", e));
                }
            }
        }

        // 3. Fallback: Check global registry
        let home = dirs::home_dir().context("No HOME")?;
        let global_sock = home.join(".cell/io").join(format!("{}.sock", target));

        if global_sock.exists() {
            let socket =
                std::os::unix::net::UnixStream::connect(&global_sock).with_context(|| {
                    format!("Failed to connect to global socket at {:?}", global_sock)
                })?;
            return Ok(socket);
        }

        anyhow::bail!(
            "IO Router down and no connection path found for '{}'. \
        Tried neighbor link at {:?} and global socket at {:?}",
            target,
            neighbor_link,
            global_sock
        )
    }

    async fn connect_to_io() -> Result<UnixStream> {
        // 1. Check env var (Support multiple routers via env injection)
        if let Ok(path) = std::env::var("CELL_ROUTER_SOCK") {
            return UnixStream::connect(path)
                .await
                .context("Env router unreachable");
        }

        // 2. Check default bootstrap
        let home = dirs::home_dir().unwrap();
        let path = home.join(".cell/io-bootstrap.sock");
        UnixStream::connect(path)
            .await
            .context("Default router unreachable")
    }

    async fn rpc(stream: &mut UnixStream, req: IoRequest) -> Result<(IoResponse, Vec<RawFd>)> {
        // 1. Send Request
        let req_bytes = cell_model::rkyv::to_bytes::<_, 1024>(&req)?.into_vec();
        let len = req_bytes.len() as u32;
        stream.write_all(&len.to_le_bytes()).await?;
        stream.write_all(&req_bytes).await?;

        // 2. Receive Response + FDs via SCM_RIGHTS
        let raw_fd = stream.as_raw_fd();
        let mut buf = [0u8; 1024];

        let (bytes_read, received_fds) = {
            let mut cmsg_buf = nix::cmsg_space!([RawFd; 4]);
            let mut iov = [IoSliceMut::new(&mut buf)];

            let msg = recvmsg::<()>(raw_fd, &mut iov, Some(&mut cmsg_buf), MsgFlags::empty())?;

            let mut fds = Vec::new();
            for cmsg in msg.cmsgs() {
                if let ControlMessageOwned::ScmRights(r) = cmsg {
                    fds.extend_from_slice(&r);
                }
            }
            (msg.bytes, fds)
        };

        let received_bytes = &buf[..bytes_read];
        let response = cell_model::rkyv::check_archived_root::<IoResponse>(received_bytes)
            .map_err(|_| anyhow::anyhow!("Invalid IO Response"))?
            .deserialize(&mut cell_model::rkyv::de::deserializers::SharedDeserializeMap::new())?;

        Ok((response, received_fds))
    }
}
