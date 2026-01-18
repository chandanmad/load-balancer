//! SHA3-512 hashing for API key secrets.

use sha3::{Digest, Sha3_512};
use uuid::Uuid;

/// Current version of the hashing algorithm.
pub const CURRENT_VERSION: i16 = 1;

/// Compute the hash for an API key.
///
/// The hash includes multiple inputs to prevent confused deputy attacks:
/// - `id`: Prevents swapping hashes between keys
/// - `version`: Prevents algorithm confusion attacks
/// - `context_id`: Prevents cross-context access (optional)
/// - `secret`: The actual secret (hashed last)
pub fn compute_hash(
    id: Uuid,
    version: i16,
    context_id: Option<Uuid>,
    secret: &[u8; 32],
) -> [u8; 64] {
    let mut hasher = Sha3_512::new();

    // Include key ID to prevent ID swapping
    hasher.update(id.as_bytes());

    // Include version to prevent algorithm confusion
    hasher.update(&version.to_le_bytes());

    // Include context ID if provided (e.g., organization_id)
    if let Some(ctx) = context_id {
        hasher.update(ctx.as_bytes());
    }

    // Include secret last (good practice to avoid length extension attacks)
    hasher.update(secret);

    let result = hasher.finalize();

    let mut hash = [0u8; 64];
    hash.copy_from_slice(&result);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_output_size() {
        let id = Uuid::new_v4();
        let secret = [42u8; 32];
        let hash = compute_hash(id, 1, None, &secret);
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_hash_deterministic() {
        let id = Uuid::new_v4();
        let secret = [42u8; 32];
        let hash1 = compute_hash(id, 1, None, &secret);
        let hash2 = compute_hash(id, 1, None, &secret);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_changes_with_id() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let secret = [42u8; 32];
        let hash1 = compute_hash(id1, 1, None, &secret);
        let hash2 = compute_hash(id2, 1, None, &secret);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_changes_with_version() {
        let id = Uuid::new_v4();
        let secret = [42u8; 32];
        let hash1 = compute_hash(id, 1, None, &secret);
        let hash2 = compute_hash(id, 2, None, &secret);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_changes_with_context() {
        let id = Uuid::new_v4();
        let ctx1 = Uuid::new_v4();
        let ctx2 = Uuid::new_v4();
        let secret = [42u8; 32];
        let hash1 = compute_hash(id, 1, Some(ctx1), &secret);
        let hash2 = compute_hash(id, 1, Some(ctx2), &secret);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hash_changes_with_secret() {
        let id = Uuid::new_v4();
        let secret1 = [42u8; 32];
        let secret2 = [43u8; 32];
        let hash1 = compute_hash(id, 1, None, &secret1);
        let hash2 = compute_hash(id, 1, None, &secret2);
        assert_ne!(hash1, hash2);
    }
}
