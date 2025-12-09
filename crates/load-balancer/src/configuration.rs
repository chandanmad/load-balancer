use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub backends: Vec<BackendConfig>,
}

#[derive(Debug, Deserialize)]
pub struct BackendConfig {
    pub prefix: String,
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

    #[test]
    fn test_deserialize_config() {
        let json_data = r#"
        {
            "backends": [
                {
                    "prefix": "/geocode/suggest",
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
                    "prefix": "/geocode/forward",
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
                    "prefix": "/geocode/reverse",
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
                    "prefix": "/geocode/reverse",
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

        assert_eq!(config.backends.len(), 4);

        // Check first backend
        let b1 = &config.backends[0];
        assert_eq!(b1.prefix, "/geocode/suggest");
        if let Backend::Hetzner { labels, port } = &b1.backend {
            assert_eq!(*port, 8099);
            assert_eq!(labels.len(), 1);
            assert_eq!(labels[0].get("env").map(|s| s.as_str()), Some("prod"));
            assert_eq!(labels[0].get("service").map(|s| s.as_str()), Some("geocode"));
        } else {
            panic!("Expected Hetzner backend");
        }

        // Check last backend
        let b4 = &config.backends[3];
        assert_eq!(b4.prefix, "/geocode/reverse");
        if let Backend::Basic { ip, port } = &b4.backend {
            assert_eq!(ip, "10.120.32.12");
            assert_eq!(*port, 8099);
        } else {
            panic!("Expected Basic backend");
        }
    }
}
