use std::sync::Arc;

use load_balancer::lb::RateLimitedLb;
use load_balancer::metric::Metrics;
use load_balancer::throttle::DummyRatelimit;

// Listeners and upstreams can be tweaked to your environment.
const LISTEN_ADDR: &str = "0.0.0.0:8080";
const UPSTREAMS: &[&str] = &["127.0.0.1:9001", "127.0.0.1:9002"];

fn main() {
    // Enable basic logging; set RUST_LOG=info for visibility.
    env_logger::init();

    let server = RateLimitedLb::start(
        LISTEN_ADDR,
        UPSTREAMS.iter().copied(),
        Arc::new(DummyRatelimit),
        Arc::new(Metrics::default()),
    )
    .expect("start load balancer");

    server.run_forever();
}
