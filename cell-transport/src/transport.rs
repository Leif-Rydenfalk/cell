// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use cell_core::{Transport, CellError, Listener, Connection, Vesicle};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use std::sync::Arc;
use core::any::Any;

pub struct UnixTransport {
    stream: Arc<Mutex<tokio::net::UnixStream>>,
}

impl UnixTransport {
    pub fn new(stream: tokio::net::UnixStream) -> Self {
        Self { stream: Arc::new(Mutex::new(stream)) }
    }
}

impl Transport for UnixTransport {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CellError>> + Send + '_>> {
        let stream_lock = self.stream.clone();
        let data_vec = data.to_vec();
        
        Box::pin(async move {
            let mut stream = stream_lock.lock().await;
            
            let len = data_vec.len() as u32;
            stream.write_all(&len.to_le_bytes()).await.map_err(|_| CellError::IoError)?;
            stream.write_all(&data_vec).await.map_err(|_| CellError::IoError)?;
            
            let mut len_buf = [0u8; 4];
            stream.read_exact(&mut len_buf).await.map_err(|_| CellError::ConnectionReset)?;
            let resp_len = u32::from_le_bytes(len_buf) as usize;

            let mut resp_buf = vec![0u8; resp_len];
            stream.read_exact(&mut resp_buf).await.map_err(|_| CellError::ConnectionReset)?;

            Ok(resp_buf)
        })
    }
}

pub struct UnixConnection {
    inner: tokio::net::UnixStream,
}

impl Connection for UnixConnection {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), CellError>> + Send + '_>> {
        Box::pin(async move {
            let mut len_buf = [0u8; 4];
            self.inner.read_exact(&mut len_buf).await.map_err(|_| CellError::ConnectionReset)?;
            let len = u32::from_le_bytes(len_buf) as usize;
            
            if len == 0 { return Err(CellError::ConnectionReset); }

            let mut buf = vec![0u8; len];
            self.inner.read_exact(&mut buf).await.map_err(|_| CellError::IoError)?;
            
            let channel = buf[0];
            let payload = buf[1..].to_vec(); 
            
            Ok((channel, Vesicle::Owned(payload)))
        })
    }

    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), CellError>> + Send + '_>> {
        let data_vec = data.to_vec();
        Box::pin(async move {
            let len = data_vec.len() as u32;
            self.inner.write_all(&len.to_le_bytes()).await.map_err(|_| CellError::IoError)?;
            self.inner.write_all(&data_vec).await.map_err(|_| CellError::IoError)?;
            Ok(())
        })
    }

    fn as_any(&mut self) -> &mut (dyn Any + Send) { self }
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