//! Configuration for API key generation and validation.

use uuid::Uuid;

/// Configuration for API key generation and validation.
#[derive(Debug, Clone)]
pub struct ApiKeyConfig {
    /// Prefix for token strings (e.g., "lb" produces "lb_v1_...").
    pub prefix: String,
    /// Optional context ID to include in hash (e.g., organization_id, account_id).
    /// Prevents hash swapping attacks between different contexts.
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

impl ApiKeyConfig {
    /// Create a new config with the given prefix.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            context_id: None,
        }
    }

    /// Set the context ID for hash binding.
    pub fn with_context(mut self, context_id: Uuid) -> Self {
        self.context_id = Some(context_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ApiKeyConfig::default();
        assert_eq!(config.prefix, "key");
        assert!(config.context_id.is_none());
    }

    #[test]
    fn test_builder_pattern() {
        let context = Uuid::new_v4();
        let config = ApiKeyConfig::new("lb").with_context(context);
        assert_eq!(config.prefix, "lb");
        assert_eq!(config.context_id, Some(context));
    }
}
