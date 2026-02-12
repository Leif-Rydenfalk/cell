// SPDX-License-Identifier: MIT
// cell-core/src/error.rs
//! Comprehensive error types for the Cell ecosystem
//!
//! This module provides structured error handling with:
//! - Hierarchical error classification
//! - Automatic error propagation via `?` operator
//! - Rich error context for debugging
//! - Error codes for programmatic handling

use core::fmt;

#[cfg(feature = "std")]
use std::error::Error as StdError;

/// Primary error classification for Cell operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum CellError {
    // Transport layer errors (100-199)
    ConnectionRefused = 100,
    ConnectionReset = 101,
    Timeout = 102,
    AccessDenied = 103,
    CapabilityMissing = 104,
    IoError = 105,
    CircuitBreakerOpen = 106,
    TransportUnavailable = 107,

    // Protocol errors (200-299)
    InvalidHeader = 200,
    SerializationFailure = 201,
    DeserializationFailure = 202,
    Corruption = 203,
    ProtocolMismatch = 204,
    InvalidMessage = 205,

    // Resource errors (300-399)
    OutOfMemory = 300,
    QuotaExceeded = 301,
    RateLimited = 302,
    ResourceExhausted = 303,

    // State errors (400-499)
    NotFound = 400,
    AlreadyExists = 401,
    InvalidState = 402,
    DependencyFailed = 403,

    // Internal errors (500-599)
    InternalError = 500,
    Panic = 501,
    NotImplemented = 502,
}

impl CellError {
    /// Get error category for classification
    pub fn category(&self) -> ErrorCategory {
        match *self {
            Self::ConnectionRefused
            | Self::ConnectionReset
            | Self::Timeout
            | Self::AccessDenied
            | Self::CapabilityMissing
            | Self::IoError
            | Self::CircuitBreakerOpen
            | Self::TransportUnavailable => ErrorCategory::Transport,

            Self::InvalidHeader
            | Self::SerializationFailure
            | Self::DeserializationFailure
            | Self::Corruption
            | Self::ProtocolMismatch
            | Self::InvalidMessage => ErrorCategory::Protocol,

            Self::OutOfMemory
            | Self::QuotaExceeded
            | Self::RateLimited
            | Self::ResourceExhausted => ErrorCategory::Resource,

            Self::NotFound | Self::AlreadyExists | Self::InvalidState | Self::DependencyFailed => {
                ErrorCategory::State
            }

            Self::InternalError | Self::Panic | Self::NotImplemented => ErrorCategory::Internal,
        }
    }

    /// Check if error is transient (can be retried)
    pub fn is_transient(&self) -> bool {
        matches!(
            self.category(),
            ErrorCategory::Transport | ErrorCategory::Resource
        )
    }

    /// Check if error indicates permanent failure
    pub fn is_permanent(&self) -> bool {
        !self.is_transient()
    }

    /// Get recommended retry delay for transient errors
    pub fn retry_delay(&self) -> Option<core::time::Duration> {
        if !self.is_transient() {
            return None;
        }

        Some(core::time::Duration::from_millis(match self {
            Self::Timeout => 100,
            Self::ConnectionRefused => 500,
            Self::ConnectionReset => 100,
            Self::CircuitBreakerOpen => 5000,
            Self::RateLimited => 1000,
            _ => 250,
        }))
    }
}

impl fmt::Display for CellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Transport
            Self::ConnectionRefused => write!(f, "Connection refused"),
            Self::ConnectionReset => write!(f, "Connection reset by peer"),
            Self::Timeout => write!(f, "Operation timed out"),
            Self::AccessDenied => write!(f, "Access denied"),
            Self::CapabilityMissing => write!(f, "Required capability not available"),
            Self::IoError => write!(f, "I/O error"),
            Self::CircuitBreakerOpen => write!(f, "Circuit breaker is open"),
            Self::TransportUnavailable => write!(f, "Transport layer unavailable"),

            // Protocol
            Self::InvalidHeader => write!(f, "Invalid message header"),
            Self::SerializationFailure => write!(f, "Serialization failed"),
            Self::DeserializationFailure => write!(f, "Deserialization failed"),
            Self::Corruption => write!(f, "Data corruption detected"),
            Self::ProtocolMismatch => write!(f, "Protocol version mismatch"),
            Self::InvalidMessage => write!(f, "Invalid message format"),

            // Resource
            Self::OutOfMemory => write!(f, "Out of memory"),
            Self::QuotaExceeded => write!(f, "Resource quota exceeded"),
            Self::RateLimited => write!(f, "Rate limit exceeded"),
            Self::ResourceExhausted => write!(f, "Resource exhausted"),

            // State
            Self::NotFound => write!(f, "Resource not found"),
            Self::AlreadyExists => write!(f, "Resource already exists"),
            Self::InvalidState => write!(f, "Invalid state for operation"),
            Self::DependencyFailed => write!(f, "Dependency check failed"),

            // Internal
            Self::InternalError => write!(f, "Internal error"),
            Self::Panic => write!(f, "Panic in cell"),
            Self::NotImplemented => write!(f, "Feature not implemented"),
        }
    }
}

#[cfg(feature = "std")]
impl StdError for CellError {}

/// Error classification categories
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Transport,
    Protocol,
    Resource,
    State,
    Internal,
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport => write!(f, "transport"),
            Self::Protocol => write!(f, "protocol"),
            Self::Resource => write!(f, "resource"),
            Self::State => write!(f, "state"),
            Self::Internal => write!(f, "internal"),
        }
    }
}

/// Rich error context for debugging and observability
#[cfg(feature = "std")]
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub code: CellError,
    pub message: String,
    pub source: Option<String>,
    pub cell: Option<String>,
    pub operation: Option<String>,
}

#[cfg(feature = "std")]
impl ErrorContext {
    pub fn new(code: CellError) -> Self {
        Self {
            code,
            message: code.to_string(),
            source: None,
            cell: None,
            operation: None,
        }
    }

    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = msg.into();
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_cell(mut self, cell: impl Into<String>) -> Self {
        self.cell = Some(cell.into());
        self
    }

    pub fn with_operation(mut self, op: impl Into<String>) -> Self {
        self.operation = Some(op.into());
        self
    }
}

#[cfg(feature = "std")]
impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code as u16, self.message)?;

        if let Some(cell) = &self.cell {
            write!(f, " (cell: {})", cell)?;
        }
        if let Some(op) = &self.operation {
            write!(f, " (op: {})", op)?;
        }
        if let Some(source) = &self.source {
            write!(f, " <- {}", source)?;
        }

        Ok(())
    }
}

#[cfg(feature = "std")]
impl StdError for ErrorContext {}

/// Result type alias for Cell operations
pub type CellResult<T> = Result<T, CellError>;
