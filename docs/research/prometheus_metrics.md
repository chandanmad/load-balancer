# Prometheus Metrics in Pingora

Guide for exporting and capturing custom Prometheus metrics.

## Setup

### Dependencies
```toml
[dependencies]
prometheus = "0.13"
lazy_static = "1.4"
```

## Metrics Server on Separate Port

```rust
use pingora::services::Service;

// Create Prometheus metrics service
let mut prometheus_service = Service::prometheus_http_service();

// Bind to port 9090 (separate from proxy traffic)
prometheus_service.add_tcp("0.0.0.0:9090");

// Add to server
my_server.add_service(prometheus_service);
```

Metrics available at: `http://localhost:9090/metrics`

## Defining Custom Metrics

```rust
use lazy_static::lazy_static;
use prometheus::{Counter, Histogram, IntCounterVec, IntGauge};
use prometheus::{register_counter, register_histogram, register_int_counter_vec, register_int_gauge};

lazy_static! {
    // Simple counter
    pub static ref REQUESTS_TOTAL: Counter = register_counter!(
        "requests_total",
        "Total number of requests"
    ).unwrap();

    // Counter with labels
    pub static ref REQUESTS_BY_STATUS: IntCounterVec = register_int_counter_vec!(
        "requests_by_status",
        "Requests by HTTP status code",
        &["status", "api_key"]
    ).unwrap();

    // Gauge (can go up/down)
    pub static ref ACTIVE_CONNECTIONS: IntGauge = register_int_gauge!(
        "active_connections",
        "Current number of active connections"
    ).unwrap();

    // Histogram for latency
    pub static ref REQUEST_LATENCY: Histogram = register_histogram!(
        "request_latency_seconds",
        "Request latency in seconds"
    ).unwrap();
}
```

## Using Metrics

```rust
// Increment counter
REQUESTS_TOTAL.inc();

// Increment with labels
REQUESTS_BY_STATUS.with_label_values(&["200", "key123"]).inc();

// Gauge operations
ACTIVE_CONNECTIONS.inc();
ACTIVE_CONNECTIONS.dec();
ACTIVE_CONNECTIONS.set(42);

// Record latency (auto timer)
let timer = REQUEST_LATENCY.start_timer();
// ... do work ...
timer.observe_duration();

// Or record manually
REQUEST_LATENCY.observe(0.025);  // 25ms
```

## Metric Types

| Type | Use Case | Goes Up/Down |
|------|----------|--------------|
| `Counter` | Total requests, errors | Up only |
| `Gauge` | Active connections, queue size | Both |
| `Histogram` | Latency, response sizes | Buckets |
| `Summary` | Percentiles (less common) | Buckets |

## Best Practices

1. **Use labels sparingly** — high cardinality labels (e.g., user IDs) cause memory bloat
2. **Prefix metrics** — e.g., `lb_requests_total` to avoid collisions
3. **Use `_total` suffix** for counters, `_seconds` for durations
4. **Register once** — use `lazy_static!` for global registration

## References

- [prometheus crate docs](https://docs.rs/prometheus)
- [Pingora observability guide](https://github.com/cloudflare/pingora)
