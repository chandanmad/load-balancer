use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::metric::Metrics;
use crate::throttle::Ratelimit;
use async_trait::async_trait;
use pingora::http::ResponseHeader;
use pingora::lb::LoadBalancer;
use pingora::lb::prelude::{RoundRobin, TcpHealthCheck};
use pingora::prelude::*;
use pingora::server::Server;
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

pub struct RateLimitedLb {
    upstreams: Arc<LoadBalancer<RoundRobin>>,
    limiter: Arc<dyn Ratelimit + Send + Sync>,
    metrics: Arc<Metrics>,
}

impl RateLimitedLb {
    pub fn new(
        upstreams: Arc<LoadBalancer<RoundRobin>>,
        limiter: Arc<dyn Ratelimit + Send + Sync>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            upstreams,
            limiter,
            metrics,
        }
    }

    /// Build and configure a pingora `Server` hosting this load balancer.
    pub fn start(
        listen_addr: &str,
        upstreams: impl IntoIterator<Item = impl Into<String>>,
        limiter: Arc<dyn Ratelimit + Send + Sync>,
        metrics: Arc<Metrics>,
    ) -> Result<Server> {
        let mut server = Server::new(None)?;
        server.bootstrap();

        let upstreams: Vec<String> = upstreams.into_iter().map(Into::into).collect();
        let mut upstream_lb: LoadBalancer<RoundRobin> = LoadBalancer::try_from_iter(upstreams)
            .map_err(|e| {
                Error::explain(ErrorType::InternalError, format!("invalid upstreams: {e}"))
            })?;
        upstream_lb.set_health_check(TcpHealthCheck::new());
        upstream_lb.health_check_frequency = Some(Duration::from_secs(5));

        // Run health checks in the background so unhealthy peers are skipped.
        let background = background_service("health check", upstream_lb);
        let upstreams = background.task();

        let mut lb_service = http_proxy_service(
            &server.configuration,
            RateLimitedLb::new(upstreams, limiter, metrics),
        );
        lb_service.add_tcp(listen_addr);

        server.add_service(background);
        server.add_service(lb_service);

        Ok(server)
    }
}

#[async_trait]
impl ProxyHttp for RateLimitedLb {
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
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let backend = self
            .upstreams
            .select(b"", 256)
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "no upstream available"))?;

        Ok(Box::new(HttpPeer::new(
            backend.addr,
            false, // plain HTTP to the upstream
            String::new(),
        )))
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
