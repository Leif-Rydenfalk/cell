// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_core::{Transport, CellError, Listener, Connection, Vesicle};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use std::sync::Arc;
use core::any::Any;
// Fixed: Proper imports for chacha20poly1305 0.10+
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use chacha20poly1305::aead::{Aead, KeyInit};

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
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CellError>> + Send + '_>> {
        let stream_lock = self.stream.clone();
        let data_vec = data.to_vec();
        
        Box::pin(async move {
            let mut stream = stream_lock.lock().await;
            
            // Length + Data (Data already contains Channel byte from Synapse)
            let len = data_vec.len() as u32;
            if stream.write_all(&len.to_le_bytes()).await.is_err() {
                return Err(CellError::IoError);
            }
            if stream.write_all(&data_vec).await.is_err() {
                return Err(CellError::IoError);
            }
            
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).await.is_err() {
                return Err(CellError::ConnectionReset);
            }
            let resp_len = u32::from_le_bytes(len_buf) as usize;

            let mut resp_buf = vec![0u8; resp_len];
            if stream.read_exact(&mut resp_buf).await.is_err() {
                return Err(CellError::ConnectionReset);
            }

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
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), CellError>> + Send + '_>> {
        Box::pin(async move {
            let mut len_buf = [0u8; 4];
            match self.inner.read_exact(&mut len_buf).await {
                Ok(_) => {},
                Err(_) => return Err(CellError::ConnectionReset),
            };
            let len = u32::from_le_bytes(len_buf) as usize;
            
            if len == 0 { return Err(CellError::ConnectionReset); }

            let mut buf = vec![0u8; len];
            match self.inner.read_exact(&mut buf).await {
                Ok(_) => {},
                Err(_) => return Err(CellError::IoError),
            };
            
            let channel = buf[0];
            let payload = buf[1..].to_vec(); 
            
            Ok((channel, Vesicle::Owned(payload)))
        })
    }

    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), CellError>> + Send + '_>> {
        let data_vec = data.to_vec();
        Box::pin(async move {
            let len = data_vec.len() as u32;
            if self.inner.write_all(&len.to_le_bytes()).await.is_err() {
                return Err(CellError::IoError);
            }
            if self.inner.write_all(&data_vec).await.is_err() {
                return Err(CellError::IoError);
            }
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
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Box<dyn Connection>, CellError>> + Send + '_>> {
        Box::pin(async move {
            match self.inner.accept().await {
                Ok((stream, _)) => Ok(Box::new(UnixConnection { inner: stream }) as Box<dyn Connection>),
                Err(_) => Err(CellError::IoError),
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
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CellError>> + Send + '_>> {
        let channel = data[0];
        let payload = data[1..].to_vec();
        let client = self.client.clone();
        
        Box::pin(async move {
            let resp_msg = client.request_raw(&payload, channel).await?;
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
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), CellError>> + Send + '_>> {
        Box::pin(async move {
            let mut spin = 0u32;
            loop {
                if let Ok(Some(msg)) = self.rx.try_read_raw() {
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

    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), CellError>> + Send + '_>> {
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

// --- Confidential Wrapper ---

pub struct SecureTransport<T: Transport> {
    inner: T,
    key: Key,
}

impl<T: Transport> SecureTransport<T> {
    pub fn new(inner: T, key_bytes: [u8; 32]) -> Self {
        Self {
            inner,
            key: *Key::from_slice(&key_bytes),
        }
    }
}

impl<T: Transport> Transport for SecureTransport<T> {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CellError>> + Send + '_>> {
        // Encrypt Request
        let cipher = ChaCha20Poly1305::new(&self.key);
        let nonce_bytes = [0u8; 12];
        let nonce = Nonce::from_slice(&nonce_bytes);
        
        // Fixed: Aead::encrypt takes payload which implements AsRef<[u8]>
        // data is &[u8], which fits.
        let encrypted_req = match cipher.encrypt(nonce, data) {
            Ok(ct) => ct,
            Err(_) => return Box::pin(async { Err(CellError::SerializationFailure) }),
        };

        // Inner Call
        let fut = self.inner.call(&encrypted_req);
        
        // Decrypt Response
        Box::pin(async move {
            let encrypted_resp = fut.await?;
            // We need a fresh cipher here because 'self' is captured by reference in the async block,
            // but ChaCha20Poly1305 is stateless/cheap to construct if we have the key.
            // Actually, we can reuse 'cipher' if we move it? No, async block lifetime issues.
            // Recreating it is safe and cheap.
            let cipher = ChaCha20Poly1305::new(&self.key);
            let nonce = Nonce::from_slice(&nonce_bytes);
            
            cipher.decrypt(nonce, encrypted_resp.as_ref())
                .map_err(|_| CellError::Corruption)
        })
    }
}