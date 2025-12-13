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
    pub const OPS: u8 = 0x02;
}

pub trait Transport: Send + Sync {
    fn call(&self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, CellError>> + Send + '_>>;
}

pub trait Connection: Send + Sync {
    fn recv(&mut self) -> Pin<Box<dyn Future<Output = Result<(u8, Vesicle<'static>), CellError>> + Send + '_>>;
    fn send(&mut self, data: &[u8]) -> Pin<Box<dyn Future<Output = Result<(), CellError>> + Send + '_>>;
    fn as_any(&mut self) -> &mut (dyn Any + Send);
}

pub trait Listener: Send + Sync {
    fn accept(&mut self) -> Pin<Box<dyn Future<Output = Result<Box<dyn Connection>, CellError>> + Send + '_>>;
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
    pub fn wrap(data: Vec<u8>) -> Self { Self::Owned(data) }
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(vec) => vec.as_slice(),
            Self::Borrowed(slice) => slice,
            Self::Guarded { data, .. } => data,
        }
    }
    pub fn len(&self) -> usize { self.as_slice().len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}