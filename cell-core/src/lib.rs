// cell-core/src/lib.rs
#![no_std]
extern crate alloc;
use alloc::vec::Vec;

pub mod channel {
    pub const APP: u8 = 0;
    // ...
}

/// The Packet Header (24 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VesicleHeader {
    pub target_id: u64, // Destination Cell ID
    pub source_id: u64, // Sender Cell ID
    pub ttl: u8,
    pub _pad: [u8; 7],
}

/// The .router file format (64 bytes)
/// Located at: .cell/routers/<TARGET_ID_HEX>.router
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RouterDescriptor {
    pub pipe_name: [u8; 32], // The filename in .cell/pipes/ to write to
    pub transport_type: u8,  // 0=File, 1=Shm... (Metadata only)
    pub _pad: [u8; 31],
}

impl RouterDescriptor {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 64 {
            return None;
        }
        unsafe {
            let mut s = core::mem::zeroed();
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), &mut s as *mut _ as *mut u8, 64);
            Some(s)
        }
    }
}
