// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_core::{Transport, TransportError, Listener, Receiver, Vesicle};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use std::sync::Arc;

// --- Unix Domain Socket Implementation ---

pub struct UnixTransport {
    stream: Arc<Mutex<tokio::net::UnixStream>>,
}

impl UnixTransport {
    pub fn new(stream: tokio::net::UnixStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
        }
    }
}

impl Transport for UnixTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>> {
        let stream_lock = self.stream.clone();
        let data_vec = data.to_vec();
        
        Box::pin(async move {
            let mut stream = stream_lock.lock().await;
            
            // Note: For Unix, we encode Channel ID into the stream manually in Synapse.
            // But wait, the previous plan was to handle it here.
            // Synapse sends [Channel, Body]. UnixTransport sends generic bytes.
            // Correct. Transport::call sends what Synapse gives it.
            
            let len = data_vec.len() as u32;
            stream.write_all(&len.to_le_bytes()).await.map_err(|_| TransportError::Io)?;
            stream.write_all(&data_vec).await.map_err(|_| TransportError::Io)?;
            
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await.map_err(|_| TransportError::Io)?;
            let resp_len = u32::from_le_bytes(len_buf) as usize;

            let mut resp_buf = vec![0u8; resp_len];
            stream.read_exact(&mut resp_buf).await.map_err(|_| TransportError::Io)?;

            Ok(resp_buf)
        })
    }
}

// Server Side Receiver Wrapper
pub struct UnixReceiver {
    inner: tokio::net::UnixStream,
}

impl Receiver for UnixReceiver {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), TransportError>> + Send + '_>> {
        Box::pin(async move {
            let mut len_buf = [0u8; 4];
            self.inner.read_exact(&mut len_buf).await.map_err(|_| TransportError::Io)?;
            let len = u32::from_le_bytes(len_buf) as usize;
            
            if len == 0 { return Err(TransportError::ConnectionClosed); }

            let mut buf = vec![0u8; len];
            self.inner.read_exact(&mut buf).await.map_err(|_| TransportError::Io)?;
            
            // Format: [Channel: 1b] + [Payload]
            let channel = buf[0];
            let payload = buf[1..].to_vec(); // Copying for simplicity/safety with 'static Vesicle requirements
            
            Ok((channel, Vesicle::Owned(payload)))
        })
    }
}

pub struct UnixListenerAdapter {
    inner: tokio::net::UnixListener,
}
impl UnixListenerAdapter {
    pub fn bind(path: impl AsRef<std::path::Path>) -> Result<Self, std::io::Error> {
        let listener = tokio::net::UnixListener::bind(path)?;
        Ok(Self { inner: listener })
    }
}
impl Listener for UnixListenerAdapter {
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Box<dyn Receiver>, TransportError>> + Send + '_>> {
        Box::pin(async move {
            match self.inner.accept().await {
                Ok((stream, _)) => Ok(Box::new(UnixReceiver { inner: stream }) as Box<dyn Receiver>),
                Err(_) => Err(TransportError::Io),
            }
        })
    }
}

// --- SHM ---
#[cfg(feature = "shm")]
pub struct ShmTransport {
    client: crate::shm::ShmClient,
}
#[cfg(feature = "shm")]
impl ShmTransport {
    pub fn new(client: crate::shm::ShmClient) -> Self { Self { client } }
}
#[cfg(feature = "shm")]
impl Transport for ShmTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>> {
        // Here data includes channel byte. We need to parse it to use ShmClient::request_raw properly.
        // Synapse encoded it.
        let channel = data[0];
        let payload = &data[1..];
        
        Box::pin(async move {
            let resp_msg = self.client.request_raw(payload, channel).await.map_err(|_| TransportError::Io)?;
            Ok(resp_msg.get_bytes().to_vec())
        })
    }
}

#[cfg(feature = "shm")]
pub struct ShmReceiver {
    // This needs internal access to a RingBuffer reader
    // For this prototype, we assume we can construct it via upgrade or other means
    // Placeholder implementation for Server-side SHM Receiver
    reader: std::sync::Arc<crate::shm::RingBuffer>, 
}

#[cfg(feature = "shm")]
impl Receiver for ShmReceiver {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), TransportError>> + Send + '_>> {
        Box::pin(async move {
            // Spin loop logic similar to ShmClient but for server
            loop {
                if let Some(msg) = self.reader.try_read_raw() {
                    let channel = msg.channel();
                    let guard = Box::new(msg.token()) as Box<dyn core::any::Any + Send + Sync>;
                    
                    return Ok((channel, Vesicle::Guarded {
                        data: msg.get_bytes(),
                        _guard: guard,
                    }));
                }
                #[cfg(feature = "std")]
                tokio::time::sleep(std::time::Duration::from_nanos(100)).await;
            }
        })
    }
}

// --- QUIC ---
#[cfg(feature = "axon")]
pub struct QuicTransport {
    connection: quinn::Connection,
}
#[cfg(feature = "axon")]
impl QuicTransport {
    pub fn new(connection: quinn::Connection) -> Self { Self { connection } }
}
#[cfg(feature = "axon")]
impl Transport for QuicTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>> {
        let conn = self.connection.clone();
        let data_vec = data.to_vec();
        Box::pin(async move {
            let (mut send, mut recv) = conn.open_bi().await.map_err(|_| TransportError::ConnectionClosed)?;
            let len = data_vec.len() as u32;
            send.write_all(&len.to_le_bytes()).await.map_err(|_| TransportError::Io)?;
            send.write_all(&data_vec).await.map_err(|_| TransportError::Io)?;
            send.finish().await.map_err(|_| TransportError::Io)?;

            let mut len_buf = [0u8; 4];
            recv.read_exact(&mut len_buf).await.map_err(|_| TransportError::Io)?;
            let resp_len = u32::from_le_bytes(len_buf) as usize;

            let mut resp_buf = vec![0u8; resp_len];
            recv.read_exact(&mut resp_buf).await.map_err(|_| TransportError::Io)?;
            Ok(resp_buf)
        })
    }
}