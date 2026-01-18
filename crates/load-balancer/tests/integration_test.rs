#![cfg(unix)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use axum::{Router, extract::Query, http::StatusCode, routing::get};
use load_balancer::accounts::hash_api_key;
use load_balancer::lb::API_KEY_HEADER;
use load_balancer::metric::Metrics;
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
use rusqlite::Connection;

/// Create a test accounts database with a plan that allows 5 requests per second.
fn create_test_accounts_db(api_key: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let conn = Connection::open(file.path()).unwrap();

    let api_key_hash = hash_api_key(api_key);

    conn.execute_batch(&format!(
        r#"
        CREATE TABLE Plans (
            plan_id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            monthly_quota INTEGER NOT NULL,
            rps_limit INTEGER NOT NULL,
            price_per_1k_req REAL NOT NULL,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE Accounts (
            account_id INTEGER PRIMARY KEY AUTOINCREMENT,
            email TEXT UNIQUE NOT NULL,
            plan_id INTEGER NOT NULL,
            billing_status TEXT NOT NULL,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (plan_id) REFERENCES Plans(plan_id)
        );
        CREATE TABLE APIKeys (
            api_key_id INTEGER PRIMARY KEY AUTOINCREMENT,
            api_key CHAR(36) UNIQUE NOT NULL,
            account_id INTEGER NOT NULL,
            api_key_hash TEXT UNIQUE NOT NULL,
            is_active BOOLEAN NOT NULL DEFAULT 1,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (account_id) REFERENCES Accounts(account_id)
        );
        CREATE TABLE ChangeLog (
            change_id INTEGER PRIMARY KEY AUTOINCREMENT,
            table_name TEXT NOT NULL,
            record_id INTEGER NOT NULL,
            operation TEXT NOT NULL,
            occurred_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );

        -- Plans triggers
        CREATE TRIGGER trg_plans_insert AFTER INSERT ON Plans BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', NEW.plan_id, 'INSERT');
        END;
        CREATE TRIGGER trg_plans_update AFTER UPDATE ON Plans BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', NEW.plan_id, 'UPDATE');
        END;
        CREATE TRIGGER trg_plans_delete AFTER DELETE ON Plans BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Plans', OLD.plan_id, 'DELETE');
        END;

        -- Accounts triggers
        CREATE TRIGGER trg_accounts_insert AFTER INSERT ON Accounts BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', NEW.account_id, 'INSERT');
        END;
        CREATE TRIGGER trg_accounts_update AFTER UPDATE ON Accounts BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', NEW.account_id, 'UPDATE');
        END;
        CREATE TRIGGER trg_accounts_delete AFTER DELETE ON Accounts BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('Accounts', OLD.account_id, 'DELETE');
        END;

        -- APIKeys triggers
        CREATE TRIGGER trg_apikeys_insert AFTER INSERT ON APIKeys BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', NEW.api_key_id, 'INSERT');
        END;
        CREATE TRIGGER trg_apikeys_update AFTER UPDATE ON APIKeys BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', NEW.api_key_id, 'UPDATE');
        END;
        CREATE TRIGGER trg_apikeys_delete AFTER DELETE ON APIKeys BEGIN
            INSERT INTO ChangeLog (table_name, record_id, operation) VALUES ('APIKeys', OLD.api_key_id, 'DELETE');
        END;

        INSERT INTO Plans (name, monthly_quota, rps_limit, price_per_1k_req)
        VALUES ('Test', 1000, 5, 0.0);

        INSERT INTO Accounts (email, plan_id, billing_status)
        VALUES ('test@example.com', 1, 'active');

        INSERT INTO APIKeys (api_key, account_id, api_key_hash, is_active)
        VALUES ('00000000-0000-0000-0000-000000000001', 1, '{}', 1);
        "#,
        api_key_hash
    ))
    .unwrap();

    file
}

fn spawn_load_balancer(
    listen_port: u16,
    config_path: String,
    accounts_db_path: String,
    metrics: Arc<Metrics>,
) -> (oneshot::Sender<()>, thread::JoinHandle<()>) {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = thread::spawn(move || {
        let listen_addr = format!("127.0.0.1:{listen_port}");

        let mut server = Server::new(None).expect("create server");

        let server_conf = ServerConfig {
            backend: config_path.clone(),
            accounts_db: accounts_db_path,
            usage_dir: None,
        };

        server
            .bootstrap(
                server_conf,
                std::path::Path::new("."),
                &listen_addr,
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
    let (_up2_addr, up2_shutdown, up2_handle) = spawn_upstream_server().await;

    let metrics = Arc::new(Metrics::default());
    let lb_port = reserve_port();

    // The API key used for testing
    let api_key = "demo-key";

    // Create test accounts database with this API key having 5 RPS limit
    let accounts_db = create_test_accounts_db(api_key);
    let accounts_db_path = accounts_db.path().to_str().unwrap().to_string();

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

    let (lb_shutdown, lb_handle) =
        spawn_load_balancer(lb_port, config_path, accounts_db_path, metrics.clone());

    wait_for_port(lb_port).await;

    let client = Client::new();
    let url = format!("http://127.0.0.1:{lb_port}/?status=200&latency_ms=5");

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

fn spawn_load_balancer_with_usage(
    listen_port: u16,
    config_path: String,
    accounts_db_path: String,
    usage_dir: String,
    metrics: Arc<Metrics>,
) -> (oneshot::Sender<()>, thread::JoinHandle<()>) {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = thread::spawn(move || {
        let listen_addr = format!("127.0.0.1:{listen_port}");

        let mut server = Server::new(None).expect("create server");

        let server_conf = ServerConfig {
            backend: config_path.clone(),
            accounts_db: accounts_db_path,
            usage_dir: Some(usage_dir),
        };

        server
            .bootstrap(
                server_conf,
                std::path::Path::new("."),
                &listen_addr,
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

#[tokio::test(flavor = "multi_thread")]
async fn usage_tracking_writes_to_sqlite_on_shutdown() {
    let (up_addr, up_shutdown, up_handle) = spawn_upstream_server().await;

    let metrics = Arc::new(Metrics::default());
    let lb_port = reserve_port();

    // The API key used for testing
    let api_key = "usage-test-key";

    // Create test accounts database
    let accounts_db = create_test_accounts_db(api_key);
    let accounts_db_path = accounts_db.path().to_str().unwrap().to_string();

    // Create usage directory
    let usage_dir = tempfile::TempDir::new().unwrap();
    let usage_dir_path = usage_dir.path().to_str().unwrap().to_string();

    // Create a temporary config file
    let mut config_file = tempfile::NamedTempFile::new().unwrap();
    let up_ip = up_addr.ip().to_string();
    let up_port = up_addr.port();

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
        up_ip, up_port
    );
    use std::io::Write;
    config_file.write_all(config_content.as_bytes()).unwrap();
    let config_path = config_file.path().to_str().unwrap().to_string();

    let (lb_shutdown, lb_handle) = spawn_load_balancer_with_usage(
        lb_port,
        config_path,
        accounts_db_path,
        usage_dir_path.clone(),
        metrics.clone(),
    );

    wait_for_port(lb_port).await;

    let client = Client::new();
    let url = format!("http://127.0.0.1:{lb_port}/?status=200&latency_ms=5");

    // Make 3 successful requests
    for _ in 0..3 {
        let resp = client
            .get(&url)
            .header(API_KEY_HEADER, api_key)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Shutdown load balancer - this should trigger usage flush
    let _ = lb_shutdown.send(());
    let _ = lb_handle.join();

    // Give a moment for file writes to complete
    sleep(Duration::from_millis(100)).await;

    // Find usage DB files
    let usage_files: Vec<_> = std::fs::read_dir(&usage_dir_path)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("usage-") && n.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();

    assert!(
        !usage_files.is_empty(),
        "Expected at least one usage-*.db file in {:?}",
        usage_dir_path
    );

    // Query the first usage DB
    let db_path = usage_files[0].path();
    let conn = Connection::open(&db_path).unwrap();

    // Verify records exist
    let total_requests: i64 = conn
        .query_row("SELECT SUM(total_requests) FROM Usage", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(
        total_requests, 3,
        "Expected 3 total requests in usage DB, got {}",
        total_requests
    );

    let _ = up_shutdown.send(());
    up_handle.await.unwrap();
}
