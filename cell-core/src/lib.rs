// cell-core/src/lib.rs
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod error;
pub mod vesicle;

pub use error::CellError;
pub use vesicle::{Vesicle, VesicleHeader};

pub mod channel {
    pub const APP: u8 = 0;
    pub const ROUTING: u8 = 1;
    pub const OPS: u8 = 2;
    pub const MACRO_COORDINATION: u8 = 3;
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RouterDescriptor {
    pub pipe_name: [u8; 32],
    pub transport_type: u8,
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
