// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use core::any::Any;

pub mod error;
pub use error::CellError;

/// Protocol Multiplexing Channels
pub mod channel {
    pub const APP: u8 = 0x00;
    pub const CONSENSUS: u8 = 0x01;
    pub const OPS: u8 = 0x02;
    pub const MACRO_COORDINATION: u8 = 0x03;
}

pub enum Wire<'a, T> {
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
    Typed(&'a T),
}

pub trait Codec {
    type Output;
    fn encode<T: ?Sized>(&self, item: &T) -> Result<Vec<u8>, CellError>;
    fn decode<'a, T: ?Sized>(&self, bytes: &'a [u8]) -> Result<Self::Output, CellError>;
}

/// Client-side: Request/Response pattern.
pub trait Transport: Send + Sync {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CellError>> + Send + '_>>;
}

/// Server-side: Zero-Copy Bidirectional Connection.
pub trait Connection: Send + Sync {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), CellError>> + Send + '_>>;
    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), CellError>> + Send + '_>>;
    fn as_any(&mut self) -> &mut (dyn Any + Send);
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send>;
}

/// Server-side: Listener.
pub trait Listener: Send + Sync {
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Box<dyn Connection>, CellError>> + Send + '_>>;
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