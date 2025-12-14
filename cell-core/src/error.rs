// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

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
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CellError {}
