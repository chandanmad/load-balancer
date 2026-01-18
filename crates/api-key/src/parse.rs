//! Token parsing for API keys.

use data_encoding::BASE32_NOPAD;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::error::{ApiKeyError, Result};
use crate::hash::CURRENT_VERSION;

/// Parsed components from a token string.
#[derive(Debug)]
pub struct ParsedToken {
    /// The UUIDv7 identifier.
    pub id: Uuid,
    /// Algorithm version.
    pub version: i16,
    /// The secret (32 bytes).
    secret: [u8; 32],
}

impl ParsedToken {
    /// Get a reference to the secret bytes.
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl Drop for ParsedToken {
    fn drop(&mut self) {
        // Clear secret from memory when dropped
        self.secret.zeroize();
    }
}

/// Parse a token string into its components.
///
/// # Arguments
/// * `token` - The full token string (e.g., "lb_v1_...")
/// * `expected_prefix` - The expected prefix (e.g., "lb")
///
/// # Returns
/// * `ParsedToken` containing id, version, and secret
/// * Error if token format is invalid
pub fn parse(token: &str, expected_prefix: &str) -> Result<ParsedToken> {
    // Split by underscore: prefix_v{version}_{payload}
    let parts: Vec<&str> = token.split('_').collect();
    if parts.len() != 3 {
        return Err(ApiKeyError::InvalidFormat);
    }

    let prefix = parts[0];
    let version_str = parts[1];
    let payload_str = parts[2];

    // Check prefix
    if prefix != expected_prefix {
        return Err(ApiKeyError::InvalidPrefix {
            expected: expected_prefix.to_string(),
            got: prefix.to_string(),
        });
    }

    // Parse version (must be "v{number}")
    let version = version_str
        .strip_prefix('v')
        .and_then(|v| v.parse::<i16>().ok())
        .ok_or(ApiKeyError::InvalidFormat)?;

    // Check version is supported
    if version != CURRENT_VERSION {
        return Err(ApiKeyError::UnsupportedVersion(version));
    }

    // Decode base32 payload (case-insensitive)
    let payload = BASE32_NOPAD
        .decode(payload_str.to_uppercase().as_bytes())
        .map_err(|_| ApiKeyError::InvalidEncoding)?;

    // Payload should be 48 bytes: UUID (16) + secret (32)
    if payload.len() != 48 {
        return Err(ApiKeyError::InvalidFormat);
    }

    // Extract UUID
    let uuid_bytes: [u8; 16] = payload[..16]
        .try_into()
        .map_err(|_| ApiKeyError::InvalidUuid)?;
    let id = Uuid::from_bytes(uuid_bytes);

    // Extract secret
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&payload[16..48]);

    Ok(ParsedToken {
        id,
        version,
        secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ApiKeyConfig;
    use crate::token::generate;

    #[test]
    fn test_parse_valid_token() {
        let config = ApiKeyConfig::new("lb");
        let generated = generate(&config);

        let parsed = parse(&generated.token, "lb").unwrap();
        assert_eq!(parsed.id, generated.id);
        assert_eq!(parsed.version, CURRENT_VERSION);
        assert_eq!(parsed.secret().len(), 32);
    }

    #[test]
    fn test_parse_invalid_prefix() {
        let config = ApiKeyConfig::new("lb");
        let generated = generate(&config);

        let result = parse(&generated.token, "wrong");
        assert!(matches!(result, Err(ApiKeyError::InvalidPrefix { .. })));
    }

    #[test]
    fn test_parse_invalid_format() {
        let result = parse("invalid_token", "lb");
        assert!(matches!(result, Err(ApiKeyError::InvalidFormat)));
    }

    #[test]
    fn test_parse_invalid_encoding() {
        // Use a string with invalid base32 characters (1, 8, 9, 0 are not in base32)
        let result = parse("lb_v1_1890", "lb");
        assert!(matches!(
            result,
            Err(ApiKeyError::InvalidEncoding) | Err(ApiKeyError::InvalidFormat)
        ));
    }
}
