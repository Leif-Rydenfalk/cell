// cell-core/src/vesicle.rs
// SPDX-License-Identifier: MIT

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

/// The header used when a packet travels through the mesh (not direct P2P).
/// This effectively implements IP-over-File-Descriptors.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VesicleHeader {
    pub target_id: u64, // Blake3 Hash of the target service name
    pub ttl: u8,        // Time To Live (hops)
    pub source_id: u64, // Blake3 Hash of sender (for replies)
    pub payload_len: u32,
    pub _reserved: [u8; 4],
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
