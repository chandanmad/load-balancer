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
use crate::usage::{UsageTracker, UsageWriter};

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

        // Setup rate limiter from accounts DB (required)
        let accounts_db_path = if std::path::Path::new(&server_conf.accounts_db).is_absolute() {
            std::path::PathBuf::from(&server_conf.accounts_db)
        } else {
            config_base_path.join(&server_conf.accounts_db)
        };

        let (account_limiter, account_service) = AccountRatelimit::from_db(&accounts_db_path)
            .map_err(|e| {
                Error::explain(
                    ErrorType::InternalError,
                    format!("failed to load accounts DB: {e}"),
                )
            })?;

        log::info!(
            "Using account-based rate limiting from {:?}",
            accounts_db_path
        );
        let account_bg = GenBackgroundService::new(
            "account data reloader".to_string(),
            Arc::new(account_service),
        );
        self.server.add_service(account_bg);

        // Setup usage tracking if configured
        let usage_tracker = if let Some(usage_dir) = &server_conf.usage_dir {
            let usage_path = if std::path::Path::new(usage_dir).is_absolute() {
                std::path::PathBuf::from(usage_dir)
            } else {
                config_base_path.join(usage_dir)
            };

            // Create directory if it doesn't exist
            std::fs::create_dir_all(&usage_path).map_err(|e| {
                Error::explain(
                    ErrorType::InternalError,
                    format!("failed to create usage directory: {e}"),
                )
            })?;

            let tracker = Arc::new(UsageTracker::new());
            let writer = UsageWriter::new(tracker.clone(), &usage_path);
            let usage_bg = GenBackgroundService::new("usage writer".to_string(), Arc::new(writer));
            self.server.add_service(usage_bg);

            log::info!("Usage tracking enabled, writing to {:?}", usage_path);
            Some(tracker)
        } else {
            None
        };

        let mut lb_service = http_proxy_service(
            &self.server.configuration,
            Lb::new(
                config_arc,
                Arc::new(account_limiter),
                metrics,
                usage_tracker,
            ),
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
