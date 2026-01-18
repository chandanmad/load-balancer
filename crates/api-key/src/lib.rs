//! Cryptographically-secure API key generation and validation.
//!
//! This crate provides functionality for:
//! - Generating secure API keys with UUIDv7 identifiers and 256-bit secrets
//! - Parsing tokens to extract their components
//! - Verifying tokens against stored hashes using constant-time comparison
//!
//! # Token Format
//!
//! Tokens follow the format: `{prefix}_v{version}_{base32(uuid || secret)}`
//!
//! Example: `lb_v1_e9n43c4499qe9a9q0zr5pj...`
//!
//! # Security Features
//!
//! - SHA3-512 hashing with context binding to prevent confused deputy attacks
//! - Constant-time comparison to prevent timing attacks
//! - Memory zeroization of secrets after use
//! - Cryptographically secure random number generation
//!
//! # Example
//!
//! ```rust
//! use api_key::{ApiKeyConfig, generate_with_data, verify};
//!
//! // Generate a new API key
//! let config = ApiKeyConfig::new("lb");
//! let (token, data) = generate_with_data(&config);
//!
//! // Give token.token to the user (only shown once!)
//! println!("Your API key: {}", token.token);
//!
//! // Store data in your database...
//!
//! // Later, verify the token
//! let is_valid = verify(&token.token, &data, &config).unwrap();
//! assert!(is_valid);
//! ```

mod config;
mod data;
mod error;
mod hash;
mod parse;
mod token;
mod verify;

// Public re-exports
pub use config::ApiKeyConfig;
pub use data::ApiKeyData;
pub use error::{ApiKeyError, Result};
pub use hash::{compute_hash, CURRENT_VERSION};
pub use parse::{parse, ParsedToken};
pub use token::{generate, generate_with_data, ApiKeyToken};
pub use verify::{verify, verify_parsed};
