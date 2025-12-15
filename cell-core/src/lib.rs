// cell-core/src/lib.rs
// SPDX-License-Identifier: MIT
// The absolute minimum primitives.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
use alloc::vec::Vec;

pub mod error;
pub use error::CellError;

/// Protocol Multiplexing Channels
pub mod channel {
    pub const APP: u8 = 0x00;
    pub const CONSENSUS: u8 = 0x01;
    pub const OPS: u8 = 0x02;
    pub const MACRO_COORDINATION: u8 = 0x03;
    // The Router Cell uses this channel to unwrap the packet
    // and send it to the next hop.
    pub const ROUTING: u8 = 0x04;
}

/// The Routing Header for Mesh traversal.
/// The SDK wraps the user payload in this if routing is required.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VesicleHeader {
    pub target_id: u64, // Blake3 Hash of target cell name
    pub source_id: u64, // Blake3 Hash of sender cell name
    pub ttl: u8,        // Time To Live
    pub _pad: [u8; 7],  // Alignment
}
