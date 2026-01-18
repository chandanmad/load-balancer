//! Token generation for API keys.

use data_encoding::BASE32_NOPAD;
use rand::RngCore;
use uuid::Uuid;

use crate::config::ApiKeyConfig;
use crate::data::ApiKeyData;
use crate::hash::{compute_hash, CURRENT_VERSION};

/// The API key token given to end users.
#[derive(Debug, Clone)]
pub struct ApiKeyToken {
    /// The full token string (prefix + version + encoded data).
    pub token: String,
    /// The UUIDv7 identifier (for database storage/lookup).
    pub id: Uuid,
}

/// Generate a new API key token.
///
/// Returns the token to give to the user. The token contains:
/// - Prefix (from config)
/// - Version (current algorithm version)
/// - Base32-encoded UUIDv7 + 32-byte secret
pub fn generate(config: &ApiKeyConfig) -> ApiKeyToken {
    let (token, _) = generate_with_data(config);
    token
}

/// Generate a new API key and return both token and storage data.
///
/// Returns:
/// - `ApiKeyToken`: The token string to give to the user
/// - `ApiKeyData`: The hash and metadata to store in the database
pub fn generate_with_data(config: &ApiKeyConfig) -> (ApiKeyToken, ApiKeyData) {
    // Generate UUIDv7 (time-ordered, random)
    let id = Uuid::now_v7();

    // Generate 32 bytes of cryptographically secure random data
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);

    // Build the payload: UUID bytes (16) + secret (32) = 48 bytes
    let mut payload = [0u8; 48];
    payload[..16].copy_from_slice(id.as_bytes());
    payload[16..].copy_from_slice(&secret);

    // Encode as lowercase base32 (no padding)
    let encoded = BASE32_NOPAD.encode(&payload).to_lowercase();

    // Build token: prefix_v{version}_{encoded}
    let token = format!("{}_v{}_{}", config.prefix, CURRENT_VERSION, encoded);

    // Compute hash for storage
    let secret_hash = compute_hash(id, CURRENT_VERSION, config.context_id, &secret);

    let api_key_token = ApiKeyToken { token, id };
    let api_key_data = ApiKeyData::new(id, secret_hash, CURRENT_VERSION);

    (api_key_token, api_key_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_token_format() {
        let config = ApiKeyConfig::new("lb");
        let token = generate(&config);

        assert!(token.token.starts_with("lb_v1_"));
        // 48 bytes -> 77 base32 chars (ceil(48 * 8 / 5))
        let parts: Vec<&str> = token.token.split('_').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "lb");
        assert_eq!(parts[1], "v1");
        assert_eq!(parts[2].len(), 77); // base32 of 48 bytes
    }

    #[test]
    fn test_generate_unique_ids() {
        let config = ApiKeyConfig::new("test");
        let token1 = generate(&config);
        let token2 = generate(&config);
        assert_ne!(token1.id, token2.id);
        assert_ne!(token1.token, token2.token);
    }

    #[test]
    fn test_generate_with_data_returns_hash() {
        let config = ApiKeyConfig::new("test");
        let (token, data) = generate_with_data(&config);

        assert_eq!(token.id, data.id);
        assert_eq!(data.version, CURRENT_VERSION);
        assert_eq!(data.secret_hash.len(), 64);
    }
}
