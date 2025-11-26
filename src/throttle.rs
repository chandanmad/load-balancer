use std::time::Duration;

/// Basic rate limit description.
pub struct Limit {
    pub quota: isize,
    pub per_seconds: u64,
}

/// Provide rate limit settings for a given API key.
pub trait Ratelimit {
    fn limit_for_key(&self, api_key: &str) -> Limit;
}

/// Default, dummy limiter that gives every key the same allowance.
pub struct DummyRatelimit;

impl Ratelimit for DummyRatelimit {
    fn limit_for_key(&self, _api_key: &str) -> Limit {
        Limit {
            quota: 5,
            per_seconds: Duration::from_secs(1).as_secs(),
        }
    }
}
