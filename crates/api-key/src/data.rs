//! Data types for API key storage.

use uuid::Uuid;

/// Data to store in database for an API key.
///
/// This contains the computed hash and metadata needed for verification.
/// The actual secret is never stored - only the hash.
#[derive(Debug, Clone)]
pub struct ApiKeyData {
    /// Unique identifier (UUIDv7, extracted from token).
    pub id: Uuid,
    /// Hash of the secret (SHA3-512, 512 bits).
    pub secret_hash: [u8; 64],
    /// Algorithm version used to generate this key.
    pub version: i16,
}

impl ApiKeyData {
    /// Create new API key data.
    pub fn new(id: Uuid, secret_hash: [u8; 64], version: i16) -> Self {
        Self {
            id,
            secret_hash,
            version,
        }
    }

    /// Get the secret hash as a hex string.
    pub fn secret_hash_hex(&self) -> String {
        self.secret_hash
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect()
    }
}
