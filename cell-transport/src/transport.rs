// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_core::{Transport, TransportError};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

/// Transport implementation for Unix Domain Sockets.
/// Used for local inter-process communication on Linux/macOS.
pub struct UnixTransport {
    stream: Mutex<tokio::net::UnixStream>,
}

impl UnixTransport {
    pub fn new(stream: tokio::net::UnixStream) -> Self {
        Self {
            stream: Mutex::new(stream),
        }
    }
}

impl Transport for UnixTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>> {
        Box::pin(async move {
            let mut stream = self.stream.lock().await;
            
            // 1. Write Length (4 bytes LE) + Data
            let len = data.len() as u32;
            stream.write_all(&len.to_le_bytes()).await.map_err(|_| TransportError::Io)?;
            stream.write_all(data).await.map_err(|_| TransportError::Io)?;
            
            // 2. Read Response Length
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await.map_err(|_| TransportError::Io)?;
            let resp_len = u32::from_le_bytes(len_buf) as usize;

            // 3. Read Response Data
            let mut resp_buf = vec![0u8; resp_len];
            stream.read_exact(&mut resp_buf).await.map_err(|_| TransportError::Io)?;

            Ok(resp_buf)
        })
    }
}

/// Transport implementation for Shared Memory Ring Buffers.
/// Used for high-performance Zero-Copy local communication.
#[cfg(feature = "shm")]
pub struct ShmTransport {
    client: crate::shm::ShmClient,
}

#[cfg(feature = "shm")]
impl ShmTransport {
    pub fn new(client: crate::shm::ShmClient) -> Self {
        Self { client }
    }
}

#[cfg(feature = "shm")]
impl Transport for ShmTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>> {
        Box::pin(async move {
            // We use the raw request method on the ShmClient.
            // This copies the result out of SHM into a Vec<u8> to satisfy the generic Transport trait.
            // Note: Optimizing this copy away requires a more complex Transport trait (e.g. returning Cow<[u8]>)
            // but strict compatibility with no_std/alloc patterns usually implies owned returns or complex lifetimes.
            let resp_msg = self.client.request_raw(data).await.map_err(|_| TransportError::Io)?;
            Ok(resp_msg.get_bytes().to_vec())
        })
    }
}

/// Transport implementation for QUIC (Axon).
/// Used for LAN/WAN communication.
#[cfg(feature = "axon")]
pub struct QuicTransport {
    connection: quinn::Connection,
}

#[cfg(feature = "axon")]
impl QuicTransport {
    pub fn new(connection: quinn::Connection) -> Self {
        Self { connection }
    }
}

#[cfg(feature = "axon")]
impl Transport for QuicTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>> {
        Box::pin(async move {
            // Open a bidirectional stream for this RPC call
            let (mut send, mut recv) = self.connection.open_bi().await.map_err(|_| TransportError::ConnectionClosed)?;
            
            // Write Request
            let len = data.len() as u32;
            send.write_all(&len.to_le_bytes()).await.map_err(|_| TransportError::Io)?;
            send.write_all(data).await.map_err(|_| TransportError::Io)?;
            send.finish().await.map_err(|_| TransportError::Io)?;

            // Read Response
            let mut len_buf = [0u8; 4];
            recv.read_exact(&mut len_buf).await.map_err(|_| TransportError::Io)?;
            let resp_len = u32::from_le_bytes(len_buf) as usize;

            let mut resp_buf = vec![0u8; resp_len];
            recv.read_exact(&mut resp_buf).await.map_err(|_| TransportError::Io)?;

            Ok(resp_buf)
        })
    }
}