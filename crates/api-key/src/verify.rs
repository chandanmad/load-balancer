//! Token verification with constant-time comparison.

use subtle::ConstantTimeEq;

use crate::config::ApiKeyConfig;
use crate::data::ApiKeyData;
use crate::error::Result;
use crate::hash::compute_hash;
use crate::parse::{parse, ParsedToken};

/// Verify a token against stored data.
///
/// This function:
/// 1. Parses the token to extract id, version, and secret
/// 2. Computes the hash using the same parameters
/// 3. Compares the computed hash against the stored hash using constant-time comparison
///
/// # Arguments
/// * `token` - The token string from the user
/// * `stored` - The stored API key data from the database
/// * `config` - Configuration with prefix and optional context_id
///
/// # Returns
/// * `Ok(true)` if the token is valid
/// * `Ok(false)` if the token is invalid (wrong secret)
/// * `Err` if the token can't be parsed
pub fn verify(token: &str, stored: &ApiKeyData, config: &ApiKeyConfig) -> Result<bool> {
    let parsed = parse(token, &config.prefix)?;
    Ok(verify_parsed(&parsed, stored, config))
}

/// Verify a pre-parsed token against stored data.
pub fn verify_parsed(parsed: &ParsedToken, stored: &ApiKeyData, config: &ApiKeyConfig) -> bool {
    // IDs must match
    if parsed.id != stored.id {
        return false;
    }

    // Versions must match
    if parsed.version != stored.version {
        return false;
    }

    // Compute hash with the same parameters
    let computed_hash = compute_hash(
        parsed.id,
        parsed.version,
        config.context_id,
        parsed.secret(),
    );

    // Constant-time comparison to prevent timing attacks
    hashes_equal(&computed_hash, &stored.secret_hash)
}

/// Constant-time comparison of two hashes.
fn hashes_equal(a: &[u8; 64], b: &[u8; 64]) -> bool {
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::generate_with_data;
    use uuid::Uuid;

    #[test]
    fn test_verify_valid_token() {
        let config = ApiKeyConfig::new("lb");
        let (token, data) = generate_with_data(&config);

        let result = verify(&token.token, &data, &config).unwrap();
        assert!(result);
    }

    #[test]
    fn test_verify_invalid_secret() {
        let config = ApiKeyConfig::new("lb");
        let (token, mut data) = generate_with_data(&config);

        // Tamper with the stored hash
        data.secret_hash[0] ^= 0xFF;

        let result = verify(&token.token, &data, &config).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_verify_wrong_context() {
        let ctx1 = Uuid::new_v4();
        let ctx2 = Uuid::new_v4();

        let config1 = ApiKeyConfig::new("lb").with_context(ctx1);
        let (token, data) = generate_with_data(&config1);

        // Verify with different context
        let config2 = ApiKeyConfig::new("lb").with_context(ctx2);
        let result = verify(&token.token, &data, &config2).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_verify_wrong_id() {
        let config = ApiKeyConfig::new("lb");
        let (token, mut data) = generate_with_data(&config);

        // Change the stored ID
        data.id = Uuid::new_v4();

        let result = verify(&token.token, &data, &config).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_roundtrip() {
        let context = Uuid::new_v4();
        let config = ApiKeyConfig::new("myapp").with_context(context);

        // Generate
        let (token, data) = generate_with_data(&config);

        // Parse (simulating database lookup by ID)
        let parsed = parse(&token.token, "myapp").unwrap();
        assert_eq!(parsed.id, data.id);

        // Verify
        let is_valid = verify(&token.token, &data, &config).unwrap();
        assert!(is_valid);
    }
}
