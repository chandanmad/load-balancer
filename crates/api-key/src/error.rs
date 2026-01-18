//! Error types for API key operations.

use thiserror::Error;

/// Errors that can occur during API key operations.
#[derive(Debug, Error)]
pub enum ApiKeyError {
    /// Token format is invalid (wrong number of parts, etc.)
    #[error("Invalid token format")]
    InvalidFormat,

    /// Token prefix doesn't match expected value
    #[error("Invalid prefix: expected '{expected}', got '{got}'")]
    InvalidPrefix { expected: String, got: String },

    /// Version number is not supported
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(i16),

    /// Base32 decoding failed
    #[error("Invalid base32 encoding")]
    InvalidEncoding,

    /// UUID extraction/parsing failed
    #[error("Invalid UUID")]
    InvalidUuid,
}

/// Result type alias for API key operations.
pub type Result<T> = std::result::Result<T, ApiKeyError>;
