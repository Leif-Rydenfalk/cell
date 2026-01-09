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

        let home = dirs::home_dir().context("No HOME dir")?;
        let io_dir = home.join(".cell/io"); // Namespace support would go here
        std::fs::create_dir_all(&io_dir)?;

        // Remove old socket if exists
        // NOTE: In a real deployment, the Router manages this to prevent port stealing.
        let sock_path = io_dir.join("in"); // Membrane usually binds to 'in' relative to CWD,
                                           // but for global addressing we adhere to the registry.

        // Wait! The previous membrane implementation bound to CWD/.cell/io/in
        // To be compatible with 'cargo run' in a specific directory:
        let cwd_sock = std::env::current_dir()?.join(".cell/io/in");
        std::fs::create_dir_all(cwd_sock.parent().unwrap())?;
        if cwd_sock.exists() {
            std::fs::remove_file(&cwd_sock)?;
        }

        let listener = std::os::unix::net::UnixListener::bind(&cwd_sock)?;
        Ok(listener)
    }

    /// Connects to the IO cell and requests a connection to a target.
    /// FALLBACK: If IO cell is down, connects directly to target socket.
    pub async fn connect(target: &str) -> Result<std::os::unix::net::UnixStream> {
        // 1. Try Router
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

        // 2. Fallback: Direct Connect
        // We assume the standard topology where neighbors are linked in .cell/neighbors/NAME/tx
        // OR we look up the global registry if that fails.

        let cwd = std::env::current_dir()?;
        let neighbor_link = cwd.join(".cell/neighbors").join(target).join("tx");

        if neighbor_link.exists() {
            let socket = std::os::unix::net::UnixStream::connect(neighbor_link)?;
            return Ok(socket);
        }

        // Last ditch: check global registry (Development mode)
        let home = dirs::home_dir().context("No HOME")?;
        // This pathing assumption needs to match how the cells bind in fallback mode.
        // If they bind to CWD, we can't easily find them unless we know their CWD.
        // However, IoClient::bind_membrane fallback above used CWD.
        // Real discovery requires the Mesh logic.
        // For 'bench' examples, they use relative paths in Cell.toml, so 'neighbor_link' should work
        // IF Organogenesis ran.

        anyhow::bail!(
            "IO Router down and direct neighbor link not found at {:?}",
            neighbor_link
        );
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
