use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use load_balancer::metric::Metrics;
use load_balancer::throttle::{DummyRatelimit, Ratelimit};
use pingora::http::ResponseHeader;
use pingora::lb::LoadBalancer;
use pingora::lb::prelude::{RoundRobin, TcpHealthCheck};
use pingora::prelude::*;
use pingora_limits::rate::Rate;

// Listeners and upstreams can be tweaked to your environment.
const LISTEN_ADDR: &str = "0.0.0.0:8080";
const UPSTREAMS: &[&str] = &["127.0.0.1:9001", "127.0.0.1:9002"];
const API_KEY_HEADER: &str = "x-api-key";
const MISSING_API_KEY: &str = "<missing>";

// Registry of Rate estimators keyed by window seconds.
static RATE_LIMITERS: OnceLock<Mutex<HashMap<u64, Arc<Rate>>>> = OnceLock::new();

struct RateLimitedLb {
    upstreams: Arc<LoadBalancer<RoundRobin>>,
    limiter: Arc<dyn Ratelimit + Send + Sync>,
    metrics: Arc<Metrics>,
}

fn rate_for_window(window_secs: u64) -> Arc<Rate> {
    let store = RATE_LIMITERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = store.lock().expect("rate limiter store poisoned");
    Arc::clone(
        guard
            .entry(window_secs)
            .or_insert_with(|| Arc::new(Rate::new(Duration::from_secs(window_secs)))),
    )
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

fn main() {
    // Enable basic logging; set RUST_LOG=info for visibility.
    env_logger::init();

    let mut server = Server::new(None).expect("server init");
    server.bootstrap();

    let mut upstreams = LoadBalancer::try_from_iter(UPSTREAMS).expect("valid upstreams");
    upstreams.set_health_check(TcpHealthCheck::new());
    upstreams.health_check_frequency = Some(Duration::from_secs(5));

    // Run health checks in the background so unhealthy peers are skipped.
    let background = background_service("health check", upstreams);
    let upstreams = background.task();

    let mut lb = http_proxy_service(
        &server.configuration,
        RateLimitedLb {
            upstreams,
            limiter: Arc::new(DummyRatelimit),
            metrics: Arc::new(Metrics::default()),
        },
    );
    lb.add_tcp(LISTEN_ADDR);

    server.add_service(background);
    server.add_service(lb);
    server.run_forever();
}
