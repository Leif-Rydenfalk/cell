// SPDX-License-Identifier: MIT
// cell-io/src/main.rs: The Circulation System
// This cell eats policy and excretes file descriptors.

use anyhow::{Result};
use cell_model::io::{IoRequest, IoResponse};
use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
use std::io::IoSlice;
use std::os::unix::io::{AsRawFd, RawFd};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    info!("[IO-Cell] Circulation System Online");

    // 1. Announce Self (Bootstrap)
    let bootstrap_path = dirs::home_dir().unwrap().join(".cell/io-bootstrap.sock");
    if bootstrap_path.exists() {
        std::fs::remove_file(&bootstrap_path).ok();
    }
    std::fs::create_dir_all(bootstrap_path.parent().unwrap())?;
    
    let listener = UnixListener::bind(&bootstrap_path)?;
    info!("[IO-Cell] Listening on {:?}", bootstrap_path);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(handle_client(stream));
            }
            Err(e) => error!("Accept error: {}", e),
        }
    }
}

async fn handle_client(mut stream: UnixStream) -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;

    let req = cell_model::rkyv::check_archived_root::<IoRequest>(&buf)
        .map_err(|_| anyhow::anyhow!("Invalid IO Request"))?;

    let req: IoRequest = req.deserialize(&mut cell_model::rkyv::de::deserializers::SharedDeserializeMap::new())?;

    match req {
        IoRequest::Bind { cell_name } => {
            info!("[IO-Cell] Request: Bind Membrane for '{}'", cell_name);
            
            let home = dirs::home_dir().unwrap();
            let io_dir = home.join(".cell/io"); 
            std::fs::create_dir_all(&io_dir)?;
            let sock_path = io_dir.join(format!("{}.sock", cell_name));
            
            if sock_path.exists() { std::fs::remove_file(&sock_path)?; }

            let listener = std::os::unix::net::UnixListener::bind(&sock_path)?;
            let fd = listener.as_raw_fd();

            send_fd(&stream, fd, &IoResponse::ListenerBound).await?;
        }
        IoRequest::Connect { target_cell } => {
            info!("[IO-Cell] Request: Connect to '{}'", target_cell);
            
            let home = dirs::home_dir().unwrap();
            let target_path = home.join(".cell/io").join(format!("{}.sock", target_cell));

            if !target_path.exists() {
                send_response(&mut stream, &IoResponse::Error { message: "Target not found".into() }).await?;
                return Ok(());
            }

            let target_conn = std::os::unix::net::UnixStream::connect(target_path)?;
            let fd = target_conn.as_raw_fd();
            send_fd(&stream, fd, &IoResponse::ConnectionEstablished).await?;
        }
    }
    Ok(())
}

async fn send_fd(stream: &UnixStream, fd: RawFd, resp: &IoResponse) -> Result<()> {
    let resp_bytes = cell_model::rkyv::to_bytes::<_, 1024>(resp)?.into_vec();
    
    let iov = [IoSlice::new(&resp_bytes)];
    let fds = [fd];
    let cmsg = ControlMessage::ScmRights(&fds);
    
    let stream_fd = stream.as_raw_fd();
    sendmsg::<()>(stream_fd, &iov, &[cmsg], MsgFlags::empty(), None)?;
    
    Ok(())
}

async fn send_response(stream: &mut UnixStream, resp: &IoResponse) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let bytes = cell_model::rkyv::to_bytes::<_, 1024>(resp)?.into_vec();
    stream.write_all(&bytes).await?;
    Ok(())
}