
// SPDX-License-Identifier: MIT
// cell-sdk/src/error_response.rs
//! Standard error response protocol for Cell IPC
//!
//! This module defines the error response structure used across all cells.
//! All error responses are serialized with rkyv for zero-copy deserialization.

use rkyv::{Archive, Deserialize, Serialize};

/// Standard error response structure for all cell communications
/// 
/// This is sent by the membrane when a request fails validation or handling.
/// Clients should check for this structure before deserializing the expected response type.
#[derive(Debug, Clone, Archive, Serialize, Deserialize, PartialEq)]
#[archive(check_bytes)]
pub struct CellErrorResponse {
    /// Error code categorizing the failure
    pub code: CellErrorCode,
    /// Human-readable error message
    pub message: String,
    /// Name of the cell that generated the error
    pub source_cell: String,
    /// Optional error details (e.g., stack trace, validation errors)
    pub details: Option<String>,
    /// Timestamp when the error occurred (Unix millis)
    pub timestamp: u64,
}

/// Error codes for cell communication failures
#[derive(Debug, Clone, Copy, Archive, Serialize, Deserialize, PartialEq, Eq)]
#[archive(check_bytes)]
#[repr(u16)]
pub enum CellErrorCode {
    /// Request deserialization/validation failed
    InvalidRequest = 1000,
    /// Handler not found for the request type
    HandlerNotFound = 1001,
    /// Request validation failed (e.g., missing fields, invalid values)
    ValidationFailed = 1002,
    
    /// Internal handler error (panic or unexpected error)
    HandlerError = 2000,
    /// Database or storage error
    StorageError = 2001,
    /// External dependency unavailable
    DependencyUnavailable = 2002,
    
    /// Cell is shutting down
    CellShuttingDown = 3000,
    /// Rate limit exceeded
    RateLimited = 3001,
    /// Resource quota exceeded
    QuotaExceeded = 3002,
    
    /// Unknown/unexpected error
    Unknown = 9999,
}

impl CellErrorCode {
    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(self, 
            CellErrorCode::DependencyUnavailable |
            CellErrorCode::RateLimited |
            CellErrorCode::StorageError
        )
    }
    
    /// Get HTTP-like status code for interoperability
    pub fn to_status_code(&self) -> u16 {
        *self as u16
    }
}

impl CellErrorResponse {
    /// Create a new error response
    pub fn new(code: CellErrorCode, message: impl Into<String>, source_cell: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source_cell: source_cell.into(),
            details: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
    
    /// Add error details
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
    
    /// Quick constructor for validation errors
    pub fn validation_failed(message: impl Into<String>, source_cell: impl Into<String>) -> Self {
        Self::new(CellErrorCode::ValidationFailed, message, source_cell)
    }
    
    /// Quick constructor for handler errors
    pub fn handler_error(message: impl Into<String>, source_cell: impl Into<String>) -> Self {
        Self::new(CellErrorCode::HandlerError, message, source_cell)
    }
    
    /// Quick constructor for invalid requests
    pub fn invalid_request(message: impl Into<String>, source_cell: impl Into<String>) -> Self {
        Self::new(CellErrorCode::InvalidRequest, message, source_cell)
    }
}

/// Result type alias for cell operations that may return errors
pub type CellResult<T> = Result<T, CellErrorResponse>;

/// Trait for types that can be converted to CellErrorResponse
pub trait IntoCellError {
    fn into_cell_error(self, source_cell: &str) -> CellErrorResponse;
}

impl IntoCellError for anyhow::Error {
    fn into_cell_error(self, source_cell: &str) -> CellErrorResponse {
        let message = format!("{:?}", self);
        CellErrorResponse::handler_error(message, source_cell)
    }
}

impl IntoCellError for std::io::Error {
    fn into_cell_error(self, source_cell: &str) -> CellErrorResponse {
        let message = format!("IO Error: {}", self);
        CellErrorResponse::new(
            if self.kind() == std::io::ErrorKind::NotFound {
                CellErrorCode::StorageError
            } else {
                CellErrorCode::Unknown
            },
            message,
            source_cell,
        )
    }
}

/// Helper to detect if bytes represent an error response
/// 
/// This checks the rkyv archive header to determine if the data
/// is a CellErrorResponse without full deserialization.
pub fn is_error_response(bytes: &[u8]) -> bool {
    // rkyv archives have a specific structure we can check
    // The first bytes contain type metadata that we can use for detection
    if bytes.len() < 8 {
        return false;
    }
    
    // Try to validate as CellErrorResponse without full deserialization
    rkyv::check_archived_root::<CellErrorResponse>(bytes).is_ok()
}

/// Deserialize either a success response or an error response
/// 
/// Returns Ok(T) on success, Err(CellErrorResponse) on error
pub fn deserialize_response<T>(bytes: &[u8]) -> Result<T, CellErrorResponse>
where
    T: Archive,
    for<'a> T::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>
        + Deserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
{
    // First check if it's an error response
    if let Ok(error_archived) = rkyv::check_archived_root::<CellErrorResponse>(bytes) {
        let error: CellErrorResponse = error_archived
            .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
            .map_err(|e| CellErrorResponse::new(
                CellErrorCode::Unknown,
                format!("Failed to deserialize error: {:?}", e),
                "unknown",
            ))?;
        return Err(error);
    }
    
    // Try to deserialize as the expected type
    let archived = rkyv::check_archived_root::<T>(bytes)
        .map_err(|e| CellErrorResponse::invalid_request(
            format!("Response validation failed: {:?}", e),
            "client",
        ))?;
    
    let result: T = archived
        .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
        .map_err(|e| CellErrorResponse::invalid_request(
            format!("Response deserialization failed: {:?}", e),
            "client",
        ))?;
    
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_error_response_roundtrip() {
        let error = CellErrorResponse::validation_failed("test error", "test_cell");
        let bytes = rkyv::to_bytes::<_, 256>(&error).unwrap();
        
        assert!(is_error_response(&bytes));
        
        let archived = rkyv::check_archived_root::<CellErrorResponse>(&bytes).unwrap();
        let deserialized: CellErrorResponse = archived
            .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
            .unwrap();
        
        assert_eq!(deserialized.code, CellErrorCode::ValidationFailed);
        assert_eq!(deserialized.message, "test error");
        assert_eq!(deserialized.source_cell, "test_cell");
    }
    
    #[test]
    fn test_error_code_is_retryable() {
        assert!(CellErrorCode::DependencyUnavailable.is_retryable());
        assert!(CellErrorCode::RateLimited.is_retryable());
        assert!(!CellErrorCode::InvalidRequest.is_retryable());
        assert!(!CellErrorCode::HandlerError.is_retryable());
    }
}
