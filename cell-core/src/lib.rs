// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![no_std]

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use core::any::Any;

/// The 20-byte immutable header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub fingerprint: u64,
    pub len: u32,
    pub crc: u32,
    pub flags: u32,
}

/// Protocol Multiplexing Channels
pub mod channel {
    pub const APP: u8 = 0x00;
    pub const CONSENSUS: u8 = 0x01;
    pub const OPS: u8 = 0x02;
}

pub enum Wire<'a, T> {
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
    Typed(&'a T),
}

pub trait Codec {
    type Output;
    fn encode<T: ?Sized>(&self, item: &T) -> Result<Vec<u8>, &'static str>;
    fn decode<'a, T: ?Sized>(&self, bytes: &'a [u8]) -> Result<Self::Output, &'static str>;
}

#[derive(Debug)]
pub enum TransportError {
    Io,
    Timeout,
    ConnectionClosed,
    Serialization,
    Other(&'static str),
}

/// Client-side: Request/Response pattern.
pub trait Transport: Send + Sync {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>>;
}

/// Server-side: Zero-Copy Bidirectional Connection.
/// Replaces Stream/Receiver to allow zero-copy reads via Vesicle.
pub trait Connection: Send + Sync {
    /// Receive a message (Channel ID, Payload).
    /// Returns a Vesicle which may be zero-copy (Guarded).
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), TransportError>> + Send + '_>>;
    
    /// Send a response.
    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + '_>>;
}

/// Server-side: Listener.
/// Abstract factory for incoming Connections.
pub trait Listener: Send + Sync {
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Box<dyn Connection>, TransportError>> + Send + '_>>;
}

/// A container for payload data (Vesicle).
pub enum Vesicle<'a> {
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
    /// Zero-copy data backed by a guard (e.g. SHM SlotToken)
    Guarded {
        data: &'a [u8],
        _guard: Box<dyn Any + Send + Sync>,
    },
    Empty,
}

impl<'a> Vesicle<'a> {
    pub fn wrap(data: Vec<u8>) -> Self {
        Self::Owned(data)
    }
    
    pub fn with_capacity(size: usize) -> Self {
        Self::Owned(alloc::vec![0u8; size])
    }
    
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(vec) => vec.as_slice(),
            Self::Borrowed(slice) => slice,
            Self::Guarded { data, .. } => data,
            Self::Empty => &[],
        }
    }
    
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(vec) => vec.as_mut_slice(),
            _ => panic!("Cannot get mutable slice from borrowed vesicle"),
        }
    }
    
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}