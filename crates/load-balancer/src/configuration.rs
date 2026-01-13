use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use pingora::services::background::BackgroundService;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub backend: String,
    /// Path to the accounts SQLite database for rate limiting.
    pub accounts_db: String,
}

#[derive(Debug)]
pub enum ConfigError {
    UndefinedService(String),
    UnusedService(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UndefinedService(s) => {
                write!(
                    f,
                    "Service '{}' referenced in backend but not defined in services",
                    s
                )
            }
            ConfigError::UnusedService(s) => {
                write!(f, "Service '{}' defined but has no backend", s)
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub services: HashMap<String, String>,
    pub backends: Vec<BackendConfig>,
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut used_services: HashSet<&String> = HashSet::new();

        for backend in &self.backends {
            if !self.services.contains_key(&backend.service) {
                return Err(ConfigError::UndefinedService(backend.service.clone()));
            }
            used_services.insert(&backend.service);
        }

        for service in self.services.keys() {
            if !used_services.contains(service) {
                return Err(ConfigError::UnusedService(service.clone()));
            }
        }

        Ok(())
    }
}

pub struct ConfigReloader {
    pub path: String,
    pub config: Arc<RwLock<Config>>,
}

#[async_trait]
impl BackgroundService for ConfigReloader {
    async fn start(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            // Check for shutdown signal
            if *shutdown.borrow() {
                return;
            }
            // Wait for 5 seconds or shutdown
            tokio::select! {
                _ = shutdown.changed() => {
                    return;
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    // Continue to reload
                }
            }

            match std::fs::read_to_string(&self.path) {
                Ok(s) => match serde_yaml::from_str::<Config>(&s) {
                    Ok(new_config) => {
                        if let Err(e) = new_config.validate() {
                            log::error!("Invalid backend config during reload: {}", e);
                        } else {
                            let mut w = self.config.write().unwrap();
                            *w = new_config;
                            log::info!("Backend config reloaded successfully");
                        }
                    }
                    Err(e) => log::error!("Failed to parse backend config during reload: {}", e),
                },
                Err(e) => log::error!("Failed to read backend config during reload: {}", e),
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct BackendConfig {
    pub service: String,
    pub backend: Backend,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Backend {
    Hetzner {
        labels: Vec<HashMap<String, String>>,
        port: u16,
    },
    Basic {
        ip: String,
        port: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;
    use serde_yaml;

    #[test]
    fn test_deserialize_config_yaml() {
        let yaml_data = r#"
        services:
          geocode_suggest: /geocode/suggest
          geocode_forward: /geocode/forward
          geocode_reverse: /geocode/reverse
        backends:
          - service: geocode_suggest
            backend:
              type: hetzner
              labels:
                - env: prod
                  service: geocode
              port: 8099
          - service: geocode_forward
            backend:
              type: hetzner
              labels:
                - env: prod
                  service: geocode
              port: 8099
          - service: geocode_reverse
            backend:
              type: hetzner
              labels:
                - env: prod
                  service: geocode
              port: 8099
          - service: geocode_reverse
            backend:
              type: basic
              ip: 10.120.32.12
              port: 8099
        "#;

        let config: Config = serde_yaml::from_str(yaml_data).expect("Failed to deserialize config");
        assert!(config.validate().is_ok());

        assert_eq!(config.services.len(), 3);
        assert_eq!(config.backends.len(), 4);

        // Check first backend
        let b1 = &config.backends[0];
        assert_eq!(b1.service, "geocode_suggest");
        assert_eq!(
            config.services.get(&b1.service).map(|s| s.as_str()),
            Some("/geocode/suggest")
        );
        if let Backend::Hetzner { labels, port } = &b1.backend {
            assert_eq!(*port, 8099);
            assert_eq!(labels.len(), 1);
            assert_eq!(labels[0].get("env").map(|s| s.as_str()), Some("prod"));
            assert_eq!(
                labels[0].get("service").map(|s| s.as_str()),
                Some("geocode")
            );
        } else {
            panic!("Expected Hetzner backend");
        }

        // Check last backend
        let b4 = &config.backends[3];
        assert_eq!(b4.service, "geocode_reverse");
        assert_eq!(
            config.services.get(&b4.service).map(|s| s.as_str()),
            Some("/geocode/reverse")
        );
        if let Backend::Basic { ip, port } = &b4.backend {
            assert_eq!(ip, "10.120.32.12");
            assert_eq!(*port, 8099);
        } else {
            panic!("Expected Basic backend");
        }
    }

    #[test]
    fn test_deserialize_config() {
        let json_data = r#"
        {
            "services": {
                "geocode_suggest": "/geocode/suggest",
                "geocode_forward": "/geocode/forward",
                "geocode_reverse": "/geocode/reverse"
            },
            "backends": [
                {
                    "service": "geocode_suggest",
                    "backend": {
                        "type": "hetzner",
                        "labels": [
                            {
                                "env": "prod",
                                "service": "geocode"
                            }
                        ],
                        "port": 8099
                    }
                },
                {
                    "service": "geocode_forward",
                    "backend": {
                        "type": "hetzner",
                        "labels": [
                            {
                                "env": "prod",
                                "service": "geocode"
                            }
                        ],
                        "port": 8099
                    }
                },
                {
                    "service": "geocode_reverse",
                    "backend": {
                        "type": "hetzner",
                        "labels": [
                            {
                                "env": "prod",
                                "service": "geocode"
                            }
                        ],
                        "port": 8099
                    }
                },
                {
                    "service": "geocode_reverse",
                    "backend": {
                        "type": "basic",
                        "ip": "10.120.32.12",
                        "port": 8099
                    }
                }
            ]
        }
        "#;

        let config: Config = serde_json::from_str(json_data).expect("Failed to deserialize config");
        assert!(config.validate().is_ok());

        assert_eq!(config.services.len(), 3);
        assert_eq!(config.backends.len(), 4);

        // Check first backend
        let b1 = &config.backends[0];
        assert_eq!(b1.service, "geocode_suggest");
        assert_eq!(
            config.services.get(&b1.service).map(|s| s.as_str()),
            Some("/geocode/suggest")
        );
        if let Backend::Hetzner { labels, port } = &b1.backend {
            assert_eq!(*port, 8099);
            assert_eq!(labels.len(), 1);
            assert_eq!(labels[0].get("env").map(|s| s.as_str()), Some("prod"));
            assert_eq!(
                labels[0].get("service").map(|s| s.as_str()),
                Some("geocode")
            );
        } else {
            panic!("Expected Hetzner backend");
        }

        // Check last backend
        let b4 = &config.backends[3];
        assert_eq!(b4.service, "geocode_reverse");
        assert_eq!(
            config.services.get(&b4.service).map(|s| s.as_str()),
            Some("/geocode/reverse")
        );
        if let Backend::Basic { ip, port } = &b4.backend {
            assert_eq!(ip, "10.120.32.12");
            assert_eq!(*port, 8099);
        } else {
            panic!("Expected Basic backend");
        }
    }

    #[test]
    fn test_validate_undefined_service() {
        let yaml_data = r#"
        services:
          geocode_suggest: /geocode/suggest
        backends:
          - service: unknown_service
            backend:
              type: basic
              ip: 10.120.32.12
              port: 8099
        "#;
        let config: Config = serde_yaml::from_str(yaml_data).expect("Failed to deserialize config");
        match config.validate() {
            Err(ConfigError::UndefinedService(s)) => assert_eq!(s, "unknown_service"),
            _ => panic!("Expected UndefinedService error"),
        }
    }

    #[test]
    fn test_validate_unused_service() {
        let yaml_data = r#"
        services:
          geocode_suggest: /geocode/suggest
          unused_service: /unused
        backends:
          - service: geocode_suggest
            backend:
              type: basic
              ip: 10.120.32.12
              port: 8099
        "#;
        let config: Config = serde_yaml::from_str(yaml_data).expect("Failed to deserialize config");
        match config.validate() {
            Err(ConfigError::UnusedService(s)) => assert_eq!(s, "unused_service"),
            _ => panic!("Expected UnusedService error"),
        }
    }
}
