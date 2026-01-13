use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use crate::accounts::Ratelimit;
use crate::configuration::{Backend, Config};
use crate::metric::Metrics;
use async_trait::async_trait;
use pingora::http::ResponseHeader;
use pingora::prelude::*;
use pingora_limits::rate::Rate;

pub const API_KEY_HEADER: &str = "x-api-key";
pub const MISSING_API_KEY: &str = "<missing>";

// Registry of Rate estimators keyed by window seconds.
static RATE_LIMITERS: OnceLock<Mutex<HashMap<u64, Arc<Rate>>>> = OnceLock::new();

fn rate_for_window(window_secs: u64) -> Arc<Rate> {
    let store = RATE_LIMITERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = store.lock().expect("rate limiter store poisoned");
    Arc::clone(
        guard
            .entry(window_secs)
            .or_insert_with(|| Arc::new(Rate::new(Duration::from_secs(window_secs)))),
    )
}

pub struct Lb {
    config: Arc<RwLock<Config>>,
    limiter: Arc<dyn Ratelimit + Send + Sync>,
    metrics: Arc<Metrics>,
}

impl Lb {
    pub fn new(
        config: Arc<RwLock<Config>>,
        limiter: Arc<dyn Ratelimit + Send + Sync>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            config,
            limiter,
            metrics,
        }
    }
}

#[async_trait]
impl ProxyHttp for Lb {
    type CTX = Option<String>;

    fn new_ctx(&self) -> Self::CTX {
        None
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        let api_key = match session
            .req_header()
            .headers
            .get(API_KEY_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            Some(k) => k.to_owned(),
            None => {
                self.metrics.record(MISSING_API_KEY, 401);
                let mut header = ResponseHeader::build(401, None)?;
                header.insert_header("WWW-Authenticate", "API key missing")?;
                session.set_keepalive(None);
                session
                    .write_response_header(Box::new(header), true)
                    .await?;
                return Ok(true);
            }
        };

        *ctx = Some(api_key.clone());

        let limit = self.limiter.limit_for_key(&api_key);
        let window_secs = limit.per_seconds.max(1);
        let rate = rate_for_window(window_secs);
        let seen = rate.observe(&api_key, 1);

        if seen > limit.quota {
            self.metrics.record(&api_key, 429);
            let mut header = ResponseHeader::build(429, None)?;
            header.insert_header("Retry-After", window_secs.to_string())?;
            header.insert_header("X-RateLimit-Limit", limit.quota.to_string())?;
            header.insert_header("X-RateLimit-Remaining", "0")?;
            session.set_keepalive(None);
            session
                .write_response_header(Box::new(header), true)
                .await?;
            return Ok(true);
        }

        Ok(false)
    }

    async fn response_filter(
        &self,
        _session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()>
    where
        Self::CTX: Send + Sync,
    {
        if let Some(api_key) = ctx.as_ref() {
            self.metrics
                .record(api_key, upstream_response.status.as_u16());
        }
        Ok(())
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let path = session.req_header().uri.path();

        let config = self.config.read().unwrap();

        // Strategy: Match path to service, then service to backend.
        // Assuming path matches the service path prefix or exact match?
        // configuration.rs: `services: HashMap<String, String>` (Name -> Path)
        // User didn't specify matching strategy, but usually it's prefix or exact.
        // Let's assume the value in services map is the prefix.

        let mut selected_service = None;
        for (service_name, service_path) in &config.services {
            if path.starts_with(service_path) {
                // simple longest match or just first match?
                // For now, let's take the first one, or maybe longest match would be better.
                // Let's stick to simple logic: match is valid.
                selected_service = Some(service_name.clone());
                break;
            }
        }

        let service_name = selected_service.ok_or_else(|| {
            Error::explain(ErrorType::HTTPStatus(404), "Service not found for path")
        })?;

        // Find backend for this service
        // config.backends is Vec<BackendConfig>.
        let backend_config = config
            .backends
            .iter()
            .find(|b| b.service == service_name)
            .ok_or_else(|| {
                Error::explain(ErrorType::HTTPStatus(503), "No backend found for service")
            })?;

        match &backend_config.backend {
            Backend::Basic { ip, port } => {
                let addr = format!("{}:{}", ip, port);
                Ok(Box::new(HttpPeer::new(
                    addr,
                    false, // plain HTTP to the upstream
                    String::new(),
                )))
            }
            Backend::Hetzner { .. } => Err(Error::explain(
                ErrorType::HTTPStatus(501),
                "Hetzner backend not implemented yet",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_for_window_reuses_same_arc_per_window() {
        let r1 = rate_for_window(1);
        let r2 = rate_for_window(1);
        let r3 = rate_for_window(2);

        assert!(Arc::ptr_eq(&r1, &r2));
        assert!(!Arc::ptr_eq(&r1, &r3));
    }
}
