use std::sync::{Arc, RwLock};

use pingora::prelude::*;
use pingora::server::RunArgs;
use pingora::server::Server as PingoraServer;
use pingora::server::configuration::Opt;
use pingora::services::background::GenBackgroundService;

use crate::accounts::AccountRatelimit;
use crate::configuration::{Config, ConfigReloader, ServerConfig};
use crate::lb::Lb;
use crate::metric::Metrics;
use crate::throttle::Ratelimit;

pub struct Server {
    server: PingoraServer,
}

impl Server {
    pub fn new(opt: Option<Opt>) -> Result<Self> {
        let server = PingoraServer::new(opt)?;
        Ok(Server { server })
    }

    pub fn bootstrap(
        &mut self,
        server_conf: ServerConfig,
        config_base_path: &std::path::Path,
        listen_addr: &str,
        fallback_limiter: Arc<dyn Ratelimit + Send + Sync>,
        metrics: Arc<Metrics>,
    ) -> Result<()> {
        self.server.bootstrap();

        let backend_config_path = if std::path::Path::new(&server_conf.backend).is_absolute() {
            std::path::PathBuf::from(&server_conf.backend)
        } else {
            config_base_path.join(&server_conf.backend)
        };

        // Initial load of backend config
        let config_str = std::fs::read_to_string(&backend_config_path).map_err(|e| {
            Error::explain(
                ErrorType::InternalError,
                format!("failed to read backend config: {e}"),
            )
        })?;
        let config: Config = serde_yaml::from_str(&config_str).map_err(|e| {
            Error::explain(
                ErrorType::InternalError,
                format!("failed to parse backend config: {e}"),
            )
        })?;
        config.validate().map_err(|e| {
            Error::explain(
                ErrorType::InternalError,
                format!("invalid backend config: {e}"),
            )
        })?;

        let config_arc = Arc::new(RwLock::new(config));

        // Background service for reloading config
        let reloader = ConfigReloader {
            path: backend_config_path.to_string_lossy().into_owned(),
            config: config_arc.clone(),
        };
        let background =
            GenBackgroundService::new("config reloader".to_string(), Arc::new(reloader));
        self.server.add_service(background);

        // Setup rate limiter - use accounts DB if provided, otherwise use fallback
        let limiter: Arc<dyn Ratelimit + Send + Sync> =
            if let Some(ref db_path) = server_conf.accounts_db {
                let accounts_db_path = if std::path::Path::new(db_path).is_absolute() {
                    std::path::PathBuf::from(db_path)
                } else {
                    config_base_path.join(db_path)
                };

                match AccountRatelimit::from_db(&accounts_db_path) {
                    Ok((account_limiter, account_service)) => {
                        log::info!(
                            "Using account-based rate limiting from {:?}",
                            accounts_db_path
                        );
                        let account_bg = GenBackgroundService::new(
                            "account data reloader".to_string(),
                            Arc::new(account_service),
                        );
                        self.server.add_service(account_bg);
                        Arc::new(account_limiter)
                    }
                    Err(e) => {
                        log::warn!("Failed to load accounts DB, using fallback limiter: {}", e);
                        fallback_limiter
                    }
                }
            } else {
                fallback_limiter
            };

        let mut lb_service = http_proxy_service(
            &self.server.configuration,
            Lb::new(config_arc, limiter, metrics),
        );

        lb_service.add_tcp(listen_addr);
        self.server.add_service(lb_service);

        Ok(())
    }

    pub fn run_forever(self) {
        self.server.run_forever();
    }

    pub fn run(self, args: RunArgs) {
        self.server.run(args);
    }
}
