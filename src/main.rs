use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use pingora::http::ResponseHeader;
use pingora::lb::LoadBalancer;
use pingora::lb::prelude::{RoundRobin, TcpHealthCheck};
use pingora::prelude::*;
use pingora_limits::rate::Rate;

// Listeners and upstreams can be tweaked to your environment.
const LISTEN_ADDR: &str = "0.0.0.0:8080";
const UPSTREAMS: &[&str] = &["127.0.0.1:9001", "127.0.0.1:9002"];

// Simple fixed-window rate limit: 5 requests per second per client IP.
const MAX_REQUESTS_PER_WINDOW: isize = 5;
const WINDOW: Duration = Duration::from_secs(1);

static RATE_LIMITER: OnceLock<Rate> = OnceLock::new();

fn rate_limiter() -> &'static Rate {
    RATE_LIMITER.get_or_init(|| Rate::new(WINDOW))
}

struct RateLimitedLb {
    upstreams: Arc<LoadBalancer<RoundRobin>>,
}

#[async_trait]
impl ProxyHttp for RateLimitedLb {
    type CTX = ();

    fn new_ctx(&self) -> Self::CTX {}

    async fn request_filter(&self, session: &mut Session, _ctx: &mut Self::CTX) -> Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        let client_ip = session
            .as_downstream()
            .client_addr()
            .and_then(|addr| addr.as_inet().map(|inet| inet.ip()));

        if let Some(ip) = client_ip {
            let seen = rate_limiter().observe(&ip, 1);
            if seen > MAX_REQUESTS_PER_WINDOW {
                let mut header = ResponseHeader::build(429, None)?;
                header.insert_header("Retry-After", WINDOW.as_secs().to_string())?;
                header.insert_header("X-RateLimit-Limit", MAX_REQUESTS_PER_WINDOW.to_string())?;
                header.insert_header("X-RateLimit-Remaining", "0")?;
                session.set_keepalive(None);
                session
                    .write_response_header(Box::new(header), true)
                    .await?;
                return Ok(true);
            }
        }

        Ok(false)
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

    let mut lb = http_proxy_service(&server.configuration, RateLimitedLb { upstreams });
    lb.add_tcp(LISTEN_ADDR);

    server.add_service(background);
    server.add_service(lb);
    server.run_forever();
}
