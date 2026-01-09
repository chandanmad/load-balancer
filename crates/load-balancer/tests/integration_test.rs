#![cfg(unix)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use axum::{Router, extract::Query, http::StatusCode, routing::get};
use load_balancer::lb::API_KEY_HEADER;
use load_balancer::metric::Metrics;
use load_balancer::throttle::DummyRatelimit;
use pingora::server::{RunArgs, ShutdownSignal, ShutdownSignalWatch};
use reqwest::Client;
use serde::Deserialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, oneshot};
use tokio::time::sleep;

#[derive(Deserialize)]
struct UpstreamParams {
    status: Option<u16>,
    latency_ms: Option<u64>,
}

async fn upstream_handler(Query(params): Query<UpstreamParams>) -> (StatusCode, String) {
    let status = params
        .status
        .and_then(|s| StatusCode::from_u16(s).ok())
        .unwrap_or(StatusCode::OK);
    if let Some(delay) = params.latency_ms {
        sleep(Duration::from_millis(delay)).await;
    }
    (status, format!("status {}", status.as_u16()))
}

async fn spawn_upstream_server() -> (SocketAddr, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let app = Router::new().route("/", get(upstream_handler));
    let server = axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    });
    let handle = tokio::spawn(async move {
        server.await.expect("upstream server failed");
    });
    (addr, shutdown_tx, handle)
}

fn reserve_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind to pick free port")
        .local_addr()
        .unwrap()
        .port()
}

struct ChannelShutdown {
    rx: Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait]
impl ShutdownSignalWatch for ChannelShutdown {
    async fn recv(&self) -> ShutdownSignal {
        if let Some(rx) = self.rx.lock().await.take() {
            let _ = rx.await;
        }
        ShutdownSignal::FastShutdown
    }
}

use load_balancer::configuration::ServerConfig;
use load_balancer::server::Server;

fn spawn_load_balancer(
    listen_port: u16,
    config_path: String,
    metrics: Arc<Metrics>,
) -> (oneshot::Sender<()>, thread::JoinHandle<()>) {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = thread::spawn(move || {
        let listen_addr = format!("127.0.0.1:{listen_port}");

        let mut server = Server::new(None).expect("create server");

        // We need to construct ServerConfig to pass to bootstrap.
        // Since we have the config path, we can read it to get the backend path?
        // Wait, the integration test passes `config_path` which points to a file created in the test.
        // This file contains:
        /*
        services: ...
        backends: ...
        */
        // It does NOT contain `backend: path/to/backend.yaml`.
        // The `Config` struct in `configuration.rs` matches this format.
        // But `Server::bootstrap` expects `ServerConfig` which has `backend: String`.
        // And then it reads `backend` path.

        // This is a disconnect.
        // In `main.rs`:
        // `conf.yaml` -> `ServerConfig` { backend: "backend.yaml" }
        // then `backend.yaml` -> `Config` { services: ..., backends: ... }

        // The integration test config file content (lines 136-144) matches `Config` struct (services, backends).
        // It does NOT match `ServerConfig`.

        // The previous `RateLimitedLb::start` took `backend_config_path`.
        // And it loaded `Config` from it.

        // My new `Server::bootstrap` takes `ServerConfig`.
        // And it uses `server_conf.backend` as the path to read `Config`.

        // So `Server::bootstrap` assumes the argument `server_conf` contains the path to the backend config.
        // In the integration test, `config_path` IS the path to the backend config (the file containing services/backends).

        // So I can simulate `ServerConfig` by creating one where `backend` is `config_path`.
        let server_conf = ServerConfig {
            backend: config_path.clone(),
        };

        // However, `Server::bootstrap` does:
        // let backend_config_path = server_conf.backend;
        // let config_str = std::fs::read_to_string(&backend_config_path)...

        // So this logic holds up. `server_conf.backend` is just a string path.

        server
            .bootstrap(
                server_conf,
                std::path::Path::new("."),
                &listen_addr,
                Arc::new(DummyRatelimit),
                metrics,
            )
            .expect("bootstrap server");

        let run_args = RunArgs {
            shutdown_signal: Box::new(ChannelShutdown {
                rx: Mutex::new(Some(shutdown_rx)),
            }),
        };

        server.run(run_args);
    });

    (shutdown_tx, handle)
}

async fn wait_for_port(port: u16) {
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..50 {
        if TcpStream::connect(&addr).await.is_ok() {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("port {addr} did not open in time");
}

fn flatten_status_counts(
    snapshot: std::collections::HashMap<u64, std::collections::HashMap<u16, u64>>,
) -> std::collections::HashMap<u16, u64> {
    let mut totals = std::collections::HashMap::new();
    for minute in snapshot.values() {
        for (code, count) in minute {
            *totals.entry(*code).or_insert(0) += *count;
        }
    }
    totals
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_and_metrics_flow_through_load_balancer() {
    let (up1_addr, up1_shutdown, up1_handle) = spawn_upstream_server().await;
    let (up2_addr, up2_shutdown, up2_handle) = spawn_upstream_server().await;

    let metrics = Arc::new(Metrics::default());
    let lb_port = reserve_port();

    // Create a temporary config file
    let mut config_file = tempfile::NamedTempFile::new().unwrap();
    let up1_ip = up1_addr.ip().to_string();
    let up1_port = up1_addr.port();

    // We only use up1 for now as our Basic backend supports single IP
    let config_content = format!(
        r#"
services:
  root: /
backends:
  - service: root
    backend:
      type: basic
      ip: "{}"
      port: {}
"#,
        up1_ip, up1_port
    );
    use std::io::Write;
    config_file.write_all(config_content.as_bytes()).unwrap();
    let config_path = config_file.path().to_str().unwrap().to_string();

    let (lb_shutdown, lb_handle) = spawn_load_balancer(lb_port, config_path, metrics.clone());

    wait_for_port(lb_port).await;

    let client = Client::new();
    let url = format!("http://127.0.0.1:{lb_port}/?status=200&latency_ms=5");
    let api_key = "demo-key";

    for _ in 0..5 {
        let resp = client
            .get(&url)
            .header(API_KEY_HEADER, api_key)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let limited = client
        .get(&url)
        .header(API_KEY_HEADER, api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

    let counts = flatten_status_counts(metrics.snapshot(api_key));
    assert_eq!(counts.get(&StatusCode::OK.as_u16()), Some(&5));
    assert_eq!(
        counts.get(&StatusCode::TOO_MANY_REQUESTS.as_u16()),
        Some(&1)
    );

    let _ = lb_shutdown.send(());
    let _ = lb_handle.join();

    let _ = up1_shutdown.send(());
    let _ = up2_shutdown.send(());
    up1_handle.await.unwrap();
    up2_handle.await.unwrap();
}
