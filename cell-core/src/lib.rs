// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![no_std]

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;

/// The 20-byte immutable header that allows Cell to scale from 
/// 10-cent MCUs to mainframes without breaking compatibility.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Header {
    /// Schema fingerprint (hash of the data structure)
    pub fingerprint: u64,
    /// Length of the payload
    pub len: u32,
    /// CRC32 of the payload (optional, 0 if unused)
    pub crc: u32,
    /// Reserved for future flags (compression, encryption bitmask)
    pub flags: u32,
}

/// Abstract representation of data on the wire.
pub enum Wire<'a, T> {
    /// Raw bytes (requires copy/deserialization)
    Owned(Vec<u8>),
    /// Zero-copy reference (e.g. Memory Mapped, DMA)
    Borrowed(&'a [u8]),
    /// Typed reference (already deserialized/cast)
    Typed(&'a T),
}

/// Trait for encoding/decoding payloads.
pub trait Codec {
    type Output;
    fn encode<T: ?Sized>(&self, item: &T) -> Result<Vec<u8>, &'static str>;
    fn decode<'a, T: ?Sized>(&self, bytes: &'a [u8]) -> Result<Self::Output, &'static str>;
}

/// Transport Error types compatible with no_std (alloc).
#[derive(Debug)]
pub enum TransportError {
    Io,
    Timeout,
    ConnectionClosed,
    Serialization,
    Other(&'static str),
}

/// The universal Transport trait.
/// Defines a Request-Response interaction pattern.
/// 
/// On embedded systems without alloc, this would use `type Future = impl Future` (TAIT),
/// but for the standard Cell ecosystem (std/alloc), we use Boxed Futures.
pub trait Transport: Send + Sync {
    /// Send a request and await a response.
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, TransportError>> + Send + '_>>;
}

/// A container for payload data (Vesicle).
pub enum Vesicle<'a> {
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
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
            Self::Empty => &[],
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::Owned(vec) => vec.as_mut_slice(),
            _ => panic!("Cannot get mutable slice from borrowed vesicle"),
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}