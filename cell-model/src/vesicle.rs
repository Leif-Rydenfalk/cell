// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use alloc::vec::Vec;
use core::ops::Deref;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use core::marker::PhantomData;

/// A container for payload data.
pub enum Vesicle<'a> {
    /// Standard heap-allocated buffer
    Owned(Vec<u8>),

    /// Zero-copy reference (e.g. Ring Buffer or Direct DMA)
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Borrowed(&'a [u8]),

    /// Fallback
    Empty,

    /// Ensures the lifetime parameter is used on platforms without SHM
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    _Phantom(PhantomData<&'a ()>),
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
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            Self::Borrowed(slice) => slice,
            Self::Empty => &[],
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            Self::_Phantom(_) => &[],
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

impl<'a> Deref for Vesicle<'a> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<'a> core::fmt::Debug for Vesicle<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Vesicle(len={})", self.len())
    }
}