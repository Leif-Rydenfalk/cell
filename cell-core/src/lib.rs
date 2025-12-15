// cell-core/src/lib.rs
// SPDX-License-Identifier: MIT
// The absolute minimum primitives.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;
use core::future::Future;
use core::pin::Pin;

pub mod error;
pub use error::CellError;

/// Protocol Multiplexing Channels
pub mod channel {
    pub const APP: u8 = 0x00;
    pub const CONSENSUS: u8 = 0x01;
    pub const OPS: u8 = 0x02;
    pub const MACRO_COORDINATION: u8 = 0x03;
    pub const ROUTING: u8 = 0x04;
}

/// The Routing Header for P2P Mesh traversal
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VesicleHeader {
    pub target_id: u64, // Blake3 Hash of target cell name
    pub source_id: u64, // Blake3 Hash of sender cell name
    pub ttl: u8,        // Time To Live
    pub _pad: [u8; 7],  // Alignment
}

pub trait Transport: Send + Sync {
    fn call(
        &self,
        data: &[u8],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CellError>> + Send + '_>>;
}

pub trait Connection: Send + Sync {
    fn recv(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), CellError>> + Send + '_>>;
    fn send(
        &mut self,
        data: &[u8],
    ) -> Pin<Box<dyn Future<Output = Result<(), CellError>> + Send + '_>>;
    fn as_any(&mut self) -> &mut (dyn Any + Send);
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send>;
}

pub trait Listener: Send + Sync {
    fn accept(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Connection>, CellError>> + Send + '_>>;
}

pub enum Vesicle<'a> {
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
    Guarded {
        data: &'a [u8],
        _guard: Box<dyn Any + Send + Sync>,
    },
}

impl<'a> Vesicle<'a> {
    pub fn wrap(data: Vec<u8>) -> Self {
        Self::Owned(data)
    }
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(vec) => vec.as_slice(),
            Self::Borrowed(slice) => slice,
            Self::Guarded { data, .. } => data,
        }
    }
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
