// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

use core::fmt;

/// Failures occurring within the Cell Substrate (Transport, Discovery, Protocol).
/// These are distinct from Application failures (defined by the user).
/// 
/// This enum is #[repr(u16)] to ensure zero-copy integer passing on the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
#[cfg_attr(feature = "rkyv", archive(check_bytes))]
#[repr(u16)]
pub enum CellError {
    // --- Transport Layer (100-199) ---
    /// The target socket/port exists but refused connection.
    ConnectionRefused = 100,
    /// The connection was established but closed unexpectedly.
    ConnectionReset = 101,
    /// The operation exceeded its deadline.
    Timeout = 102,
    /// The circuit breaker is open; request rejected fast.
    CircuitBreakerOpen = 103,
    /// No route to the target cell (Discovery failed).
    Unreachable = 104,
    /// Generic IO error from the OS.
    IoError = 105,

    // --- Protocol Layer (200-299) ---
    /// The received message header was invalid or corrupted.
    InvalidHeader = 200,
    /// The payload checksum (CRC) did not match.
    Corruption = 201,
    /// The target speaks a different protocol version.
    VersionMismatch = 202,
    /// Serialization/Deserialization failed (Type mismatch).
    SerializationFailure = 203,
    /// The received vesicle was empty.
    EmptyPayload = 204,

    // --- Access Control (300-399) ---
    /// The SHM handshake failed (UID mismatch or invalid token).
    AccessDenied = 300,
    /// The cell is in a simplified mode and cannot handle this request.
    CapabilityMissing = 301,

    // --- Resource (400-499) ---
    /// The target cell is shedding load (backpressure).
    Overloaded = 400,
    /// Shared memory ring buffer is full.
    OutboundBufferFull = 401,
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CellError {}