// SPDX-License-Identifier: MIT
// cell-core/src/error.rs

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CellError {
    ConnectionRefused = 100,
    ConnectionReset = 101,
    Timeout = 102,
    AccessDenied = 103,
    CapabilityMissing = 104,
    IoError = 105,
    CircuitBreakerOpen = 106,

    InvalidHeader = 200,
    SerializationFailure = 203,
    Corruption = 204,
    ProtocolMismatch = 205,
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CellError::ConnectionRefused => write!(f, "Connection Refused"),
            CellError::ConnectionReset => write!(f, "Connection Reset"),
            CellError::Timeout => write!(f, "Timeout"),
            CellError::AccessDenied => write!(f, "Access Denied"),
            CellError::CapabilityMissing => write!(f, "Capability Missing"),
            CellError::IoError => write!(f, "I/O Error"),
            CellError::CircuitBreakerOpen => write!(f, "Circuit Breaker Open"),
            CellError::InvalidHeader => write!(f, "Invalid Vesicle Header"),
            CellError::SerializationFailure => write!(f, "Serialization Failure"),
            CellError::Corruption => write!(f, "Data Corruption Detected"),
            CellError::ProtocolMismatch => write!(f, "Protocol Mismatch"),
        }
    }
}

// Now this works because `extern crate std` is conditional in lib.rs
#[cfg(feature = "std")]
impl std::error::Error for CellError {}
