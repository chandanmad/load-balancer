/// A collection of upstream endpoints (ip:port pairs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    pub endpoints: Vec<String>,
}

impl Upstream {
    pub fn new(endpoints: Vec<String>) -> Self {
        Upstream { endpoints }
    }
}

/// Supplies upstream endpoints to the load balancer.
pub trait UpstreamsProvider {
    fn upstreams(&self) -> Upstream;
}

/// Simple static provider backed by a fixed list of endpoints.
pub struct StaticUpstreams {
    endpoints: Vec<String>,
}

impl StaticUpstreams {
    pub fn new(endpoints: Vec<String>) -> Self {
        StaticUpstreams { endpoints }
    }
}

impl UpstreamsProvider for StaticUpstreams {
    fn upstreams(&self) -> Upstream {
        Upstream::new(self.endpoints.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_holds_endpoints() {
        let endpoints = vec!["127.0.0.1:8080".to_string(), "127.0.0.1:8081".to_string()];
        let upstream = Upstream::new(endpoints.clone());
        assert_eq!(upstream.endpoints, endpoints);
    }

    #[test]
    fn static_provider_clones_endpoints() {
        let provider = StaticUpstreams::new(vec!["10.0.0.1:80".into(), "10.0.0.2:80".into()]);
        let upstream = provider.upstreams();
        assert_eq!(
            upstream.endpoints,
            vec!["10.0.0.1:80".to_string(), "10.0.0.2:80".to_string()]
        );
    }
}
