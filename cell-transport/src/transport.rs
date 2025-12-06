// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_core::{Transport, TransportError, Listener, Connection, Vesicle};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use std::sync::Arc;
use core::any::Any;

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

// Client Side
impl Transport for UnixTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>> {
        let stream_lock = self.stream.clone();
        let data_vec = data.to_vec();
        
        Box::pin(async move {
            let mut stream = stream_lock.lock().await;
            
            // Length + Data (Data already contains Channel byte from Synapse)
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

// Server Side Connection
pub struct UnixConnection {
    inner: tokio::net::UnixStream,
}

impl UnixConnection {
    pub fn into_inner(self) -> tokio::net::UnixStream {
        self.inner
    }
}

impl Connection for UnixConnection {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), TransportError>> + Send + '_>> {
        Box::pin(async move {
            let mut len_buf = [0u8; 4];
            self.inner.read_exact(&mut len_buf).await.map_err(|_| TransportError::Io)?;
            let len = u32::from_le_bytes(len_buf) as usize;
            
            if len == 0 { return Err(TransportError::ConnectionClosed); }

            let mut buf = vec![0u8; len];
            self.inner.read_exact(&mut buf).await.map_err(|_| TransportError::Io)?;
            
            let channel = buf[0];
            let payload = buf[1..].to_vec(); 
            
            Ok((channel, Vesicle::Owned(payload)))
        })
    }

    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + '_>> {
        let data_vec = data.to_vec();
        Box::pin(async move {
            let len = data_vec.len() as u32;
            self.inner.write_all(&len.to_le_bytes()).await.map_err(|_| TransportError::Io)?;
            self.inner.write_all(&data_vec).await.map_err(|_| TransportError::Io)?;
            Ok(())
        })
    }

    fn as_any(&mut self) -> &mut (dyn Any + Send) {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
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
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Box<dyn Connection>, TransportError>> + Send + '_>> {
        Box::pin(async move {
            match self.inner.accept().await {
                Ok((stream, _)) => Ok(Box::new(UnixConnection { inner: stream }) as Box<dyn Connection>),
                Err(_) => Err(TransportError::Io),
            }
        })
    }
}

// --- SHM Implementation ---

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
        let channel = data[0];
        let payload = data[1..].to_vec();
        let client = self.client.clone();
        
        Box::pin(async move {
            let resp_msg = client.request_raw(&payload, channel).await.map_err(|_| TransportError::Io)?;
            Ok(resp_msg.get_bytes().to_vec())
        })
    }
}

#[cfg(feature = "shm")]
pub struct ShmConnection {
    rx: std::sync::Arc<crate::shm::RingBuffer>,
    tx: std::sync::Arc<crate::shm::RingBuffer>,
}

#[cfg(feature = "shm")]
impl ShmConnection {
    pub fn new(rx: std::sync::Arc<crate::shm::RingBuffer>, tx: std::sync::Arc<crate::shm::RingBuffer>) -> Self {
        Self { rx, tx }
    }
}

#[cfg(feature = "shm")]
impl Connection for ShmConnection {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), TransportError>> + Send + '_>> {
        Box::pin(async move {
            let mut spin = 0u32;
            loop {
                if let Some(msg) = self.rx.try_read_raw() {
                    let channel = msg.channel();
                    let data_ptr = msg.get_bytes();
                    let guard = Box::new(msg.token()) as Box<dyn core::any::Any + Send + Sync>;
                    
                    return Ok((channel, Vesicle::Guarded {
                        data: data_ptr,
                        _guard: guard,
                    }));
                }
                
                spin += 1;
                if spin < 10000 {
                    std::hint::spin_loop();
                } else {
                    #[cfg(feature = "std")]
                    tokio::time::sleep(std::time::Duration::from_nanos(100)).await;
                    spin = 0;
                }
            }
        })
    }

    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + '_>> {
        let data_vec = data.to_vec();
        Box::pin(async move {
            let size = data_vec.len();
            let mut slot = self.tx.wait_for_slot(size).await;
            slot.write(&data_vec, 0); 
            slot.commit(size);
            Ok(())
        })
    }

    fn as_any(&mut self) -> &mut (dyn Any + Send) { self }
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> { self }
}

// --- QUIC Implementation ---

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