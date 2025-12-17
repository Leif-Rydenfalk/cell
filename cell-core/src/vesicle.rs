// cell-core/src/vesicle.rs
// SPDX-License-Identifier: MIT

use alloc::vec;
use alloc::vec::Vec;

/// The Universal Packet Header (24 Bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VesicleHeader {
    pub target_id: u64, // Blake3 Hash of target cell name
    pub source_id: u64, // Blake3 Hash of sender cell name (for replies)
    pub ttl: u8,        // Hops remaining
    pub flags: u8,      // Reserved (0x01 = Fragment, 0x02 = Ack...)
    pub _pad: [u8; 6],  // Alignment to 24 bytes
}

impl VesicleHeader {
    pub const SIZE: usize = 24;
}

/// A wrapper around a data buffer.
#[derive(Debug, Clone)]
pub struct Vesicle {
    data: Vec<u8>,
}

impl Vesicle {
    pub fn wrap(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: vec![0; capacity],
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }
}
