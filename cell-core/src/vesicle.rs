// cell-core/src/vesicle.rs
// SPDX-License-Identifier: MIT

use alloc::vec::Vec;
use core::any::Any;

/// The Universal Packet Header.
/// Used for routing between cells via the SDK's file pipes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VesicleHeader {
    pub target_id: u64, // Blake3 Hash of target cell name
    pub source_id: u64, // Blake3 Hash of sender cell name (for replies)
    pub ttl: u8,        // Hops remaining
    pub flags: u8,      // Reserved
    pub _pad: [u8; 6],  // Alignment to 24 bytes
}

impl VesicleHeader {
    pub const SIZE: usize = 24;
}
