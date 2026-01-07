use std::sync::{Arc, RwLock};

use pingora::prelude::*;
use pingora::server::RunArgs;
use pingora::server::Server as PingoraServer;
use pingora::server::configuration::Opt;
use pingora::services::background::GenBackgroundService;

use crate::configuration::{Config, ConfigReloader, ServerConfig};
use crate::lb::RateLimitedLb;
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
        listen_addr: &str,
        limiter: Arc<dyn Ratelimit + Send + Sync>,
        metrics: Arc<Metrics>,
    ) -> Result<()> {
        self.server.bootstrap();

        let backend_config_path = server_conf.backend;

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
            path: backend_config_path.clone(),
            config: config_arc.clone(),
        };
        let background =
            GenBackgroundService::new("config reloader".to_string(), Arc::new(reloader));

        let mut lb_service = http_proxy_service(
            &self.server.configuration,
            RateLimitedLb::new(config_arc, limiter, metrics),
        );

        lb_service.add_tcp(listen_addr);

        self.server.add_service(background);
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
