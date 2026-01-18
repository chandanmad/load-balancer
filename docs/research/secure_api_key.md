# Secure API Key Crate - Research Document

> Based on [How to implement cryptographically-secure API keys in Rust](https://kerkour.com/api-keys) by Sylvain Kerkour

## Overview

This document outlines the design for a new `api-key` crate that provides cryptographically-secure API key generation and validation. The crate will be independent and reusable, with no database dependencies - it only handles the cryptographic operations.

## Goals

1. **Generate secure API keys** - Create tokens that can be given to end users
2. **Validate API keys** - Given a token and stored hash, verify the token is valid
3. **Database-agnostic** - No SQLite/Postgres dependencies; consumers store the hash however they want
4. **Human-friendly format** - Easily recognizable, double-click selectable, with service prefix
5. **Security scanner friendly** - Format detectable by automated secret scanners (e.g., GitHub)

---

## API Key Format

```
Token = Prefix + Version + base32_lowercase(UUIDv7 || Secret)
```

### Components

| Component | Size | Purpose |
|-----------|------|---------|
| **Prefix** | Variable | Service identifier (e.g., `lb_` for load-balancer) |
| **Version** | 2 chars | Algorithm version for future evolution (e.g., `v1`) |
| **UUIDv7** | 16 bytes | Unique ID, extractable for database lookup |
| **Secret** | 32 bytes | Cryptographically-secure random data (256 bits) |

### Example Tokens

```
lb_v1_e9n43c4499qe9a9q0zr5pj7abc123...
```

The underscore separators make the token:
- Easy to visually parse
- Double-click selectable in terminals/editors
- Recognizable by secret scanners

---

## Data Structures

### Token (Given to User)

The full API key string that users store and send in requests:

```rust
/// The API key token given to end users
pub struct ApiKeyToken {
    /// The full token string (prefix + version + encoded data)
    pub token: String,
    /// Extracted UUIDv7 (for database storage/lookup)
    pub id: Uuid,
}
```

### Stored Data (In Database)

What the application stores in its database:

```rust
/// Data to store in database for an API key
pub struct ApiKeyData {
    /// Unique identifier (UUIDv7, extracted from token)
    pub id: Uuid,
    /// Hash of the secret (512 bits)
    pub secret_hash: [u8; 64],
    /// Algorithm version used
    pub version: i16,
}
```

### Parsed Token (Internal)

Used during validation:

```rust
/// Parsed components from a token string
struct ParsedToken {
    id: Uuid,
    version: i16,
    secret: [u8; 32],
}
```

---

## Hashing Strategy

### Why Not Just Hash the Secret?

Hashing only the secret is vulnerable to the **confused deputy attack**:
- Attacker with database access could swap hash A with hash B
- Attacker knows secret for key A, but now it validates as key B
- Attacker gains access to a different organization's resources

### Solution: Include Context in Hash

```rust
fn hash_api_key(
    api_key_id: Uuid,
    version: i16,
    context_id: Uuid,  // e.g., organization_id, account_id
    secret: &[u8; 32],
) -> [u8; 64] {
    let mut hasher = Sha3_512::new();
    
    hasher.update(api_key_id.as_bytes());      // Prevents ID swapping
    hasher.update(&version.to_le_bytes());      // Prevents algorithm confusion
    hasher.update(context_id.as_bytes());       // Prevents context swapping
    hasher.update(secret);                      // The actual secret (last!)
    
    hasher.finalize().into()
}
```

### Hash Function Choice

Any of these are suitable (all provide 256+ bits of security):

| Function | Output Size | Notes |
|----------|-------------|-------|
| SHA3-512 | 512 bits | NIST standard, post-quantum resistant |
| SHA-512 | 512 bits | Widely available, well-audited |
| BLAKE3 | Variable | Very fast, modern design |
| SHAKE256 | Variable | Extendable output, NIST standard |

> **Note:** We do NOT use password hashing (Argon2id, bcrypt) because the secret is already cryptographically random. Password hashing is for low-entropy human passwords.

---

## Public API Design

### Configuration

```rust
/// Configuration for API key generation/validation
pub struct ApiKeyConfig {
    /// Prefix for tokens (e.g., "lb" -> "lb_v1_...")
    pub prefix: String,
    /// Optional context ID to include in hash (organization_id, tenant_id, etc.)
    pub context_id: Option<Uuid>,
}

impl Default for ApiKeyConfig {
    fn default() -> Self {
        Self {
            prefix: "key".to_string(),
            context_id: None,
        }
    }
}
```

### Generation

```rust
/// Generate a new API key
pub fn generate(config: &ApiKeyConfig) -> ApiKeyToken;

/// Generate and return both token and storage-ready data
pub fn generate_with_data(config: &ApiKeyConfig) -> (ApiKeyToken, ApiKeyData);
```

### Validation

```rust
/// Parse a token string into components (for database lookup)
pub fn parse(token: &str, expected_prefix: &str) -> Result<ParsedToken, ApiKeyError>;

/// Verify a token against stored hash
/// Returns true if valid, false if invalid
pub fn verify(
    token: &str,
    stored: &ApiKeyData,
    config: &ApiKeyConfig,
) -> Result<bool, ApiKeyError>;

/// Compute hash for a parsed token (for manual comparison)
pub fn compute_hash(
    parsed: &ParsedToken,
    context_id: Option<Uuid>,
) -> [u8; 64];
```

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum ApiKeyError {
    #[error("Invalid token format")]
    InvalidFormat,
    
    #[error("Invalid prefix: expected '{expected}', got '{got}'")]
    InvalidPrefix { expected: String, got: String },
    
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(i16),
    
    #[error("Invalid base32 encoding")]
    InvalidEncoding,
    
    #[error("Invalid UUID")]
    InvalidUuid,
}
```

---

## Security Considerations

### Constant-Time Comparison

When comparing hashes, always use constant-time comparison to prevent timing attacks:

```rust
use subtle::ConstantTimeEq;

fn hashes_equal(a: &[u8; 64], b: &[u8; 64]) -> bool {
    a.ct_eq(b).into()
}
```

### Memory Zeroization

Secrets should be cleared from memory after use:

```rust
use zeroize::Zeroize;

impl Drop for ParsedToken {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}
```

### Cryptographically Secure RNG

Always use `rand::rngs::OsRng` or equivalent:

```rust
use rand::RngCore;

let mut secret = [0u8; 32];
rand::rngs::OsRng.fill_bytes(&mut secret);
```

---

## Crate Structure

```
crates/
└── api-key/
    ├── Cargo.toml
    └── src/
        ├── lib.rs          # Public API, re-exports
        ├── config.rs       # ApiKeyConfig
        ├── token.rs        # ApiKeyToken, generation
        ├── data.rs         # ApiKeyData, storage types
        ├── parse.rs        # Token parsing
        ├── hash.rs         # Hashing logic
        ├── verify.rs       # Verification logic
        └── error.rs        # ApiKeyError
```

### Dependencies

```toml
[package]
name = "api-key"
version = "0.1.0"
edition = "2024"

[dependencies]
uuid = { version = "1", features = ["v7"] }
sha3 = "0.10"                    # SHA3-512 hashing
data-encoding = "2"             # base32 encoding
rand = "0.8"                    # Secure random generation
subtle = "2"                    # Constant-time comparison
zeroize = { version = "1", features = ["derive"] }  # Memory clearing
thiserror = "2"                 # Error handling
```

---

## Usage Example

### Creating an API Key

```rust
use api_key::{ApiKeyConfig, generate_with_data};
use uuid::Uuid;

// Configure with service prefix and organization context
let config = ApiKeyConfig {
    prefix: "lb".to_string(),
    context_id: Some(organization_id),
};

// Generate new API key
let (token, data) = generate_with_data(&config);

// Give token.token to the user (only shown once!)
println!("Your API key: {}", token.token);

// Store data in database
db.insert_api_key(ApiKey {
    id: data.id,
    secret_hash: data.secret_hash,
    version: data.version,
    organization_id,
    // ... other fields
});
```

### Validating an API Key

```rust
use api_key::{ApiKeyConfig, parse, verify};

// User sends token in Authorization header
let token_str = "lb_v1_e9n43c4499qe9a9q0zr5pj...";

// Parse to get ID for database lookup
let parsed = parse(token_str, "lb")?;

// Fetch stored data from database
let stored = db.find_api_key_by_id(parsed.id)?;

// Verify token against stored hash
let config = ApiKeyConfig {
    prefix: "lb".to_string(),
    context_id: Some(stored.organization_id),
};

if verify(token_str, &stored, &config)? {
    // Token is valid, grant access
} else {
    // Token is invalid, reject request
}
```

---

## Integration with load-balancer Crate

The `api-key` crate will be used by `load-balancer` for:

1. **API key validation at request time**
   - Currently uses simple SHA-256 hash of the key
   - Will be upgraded to use `api_key::verify()`

2. **Migration path**
   - Add version field to APIKeys table
   - Support both old (v0) and new (v1) formats during transition
   - New keys generated with v1 format

### Schema Changes

```sql
ALTER TABLE APIKeys ADD COLUMN version SMALLINT NOT NULL DEFAULT 0;
ALTER TABLE APIKeys ADD COLUMN secret_hash BLOB;  -- 64 bytes for SHA3-512
-- api_key_hash remains for backward compatibility with v0
```

---

## Future Enhancements

1. **Key rotation** - Generate new secret while keeping same ID
2. **Scoped keys** - Embed permissions in the token
3. **Key prefixes** - Support environment indicators (`lb_test_v1_...`, `lb_live_v1_...`)
4. **Expiration encoding** - Optionally embed expiry in token to reject without DB lookup
5. **Rate limit tiers** - Embed rate limit tier in token

---

## References

- [How to implement cryptographically-secure API keys in Rust](https://kerkour.com/api-keys)
- [SHA-256 Length Extension Attacks](https://kerkour.com/sha256-length-extension-attacks)
- [NIST SP 800-185 - SHA-3 Derived Functions](https://csrc.nist.gov/publications/detail/sp/800-185/final)
- [UUID Version 7 Specification](https://www.ietf.org/archive/id/draft-peabody-dispatch-new-uuid-format-04.html)
