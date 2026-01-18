# Pingora Background Thread Configuration

Research on configuring Pingora background services to minimize request latency impact.

## Configuration Settings

### pingora.conf

```yaml
threads: 4            # Number of worker threads
work_stealing: false  # Thread scheduling model (default: true)
```

## Thread Models

| Model | Setting | Behavior | Best For |
|-------|---------|----------|----------|
| **Work-stealing** | `work_stealing: true` | Tasks can migrate between threads for load balancing | Bursty, uneven loads |
| **Thread-per-core** | `work_stealing: false` | Tasks stay on assigned thread, no cross-thread migration | Predictable workloads, lower latency jitter |

## Key Facts

- **Threads are not shared between services** — each Pingora service gets its own thread pool
- Pingora uses Tokio async runtime under the hood
- Cloudflare observed **5ms reduction in median TTFB** switching to Pingora

## Recommendations for Background Services

### 1. Keep background work lightweight
- Use async I/O (already doing this with `rusqlite`)
- Poll infrequently (30-second intervals for account refresh)

### 2. Use `spawn_blocking` for heavy work
```rust
let result = tokio::task::spawn_blocking(|| {
    // CPU-intensive or blocking I/O here
}).await;
```

### 3. Avoid blocking the async runtime
- Don't hold locks across `.await` points
- Use `RwLock` over `Mutex` when reads dominate

## Current Implementation Status

Our `AccountDataService` and `UsageWriter` are I/O-bound and use async/await, which coexists well with request processing on Pingora's runtime.

## References

- [Cloudflare Pingora Blog](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/)
- [Pingora GitHub](https://github.com/cloudflare/pingora)
