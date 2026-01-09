use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, Query},
    routing::{self},
};
use clap::Parser;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

#[derive(Parser, Debug)]
#[command(name = "upstream", long_about = None)]
struct Args {
    #[arg(short, long)]
    port: i32,

    #[arg(short, long)]
    ip: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    println!("{:?}", args);

    let listener = TcpListener::bind(format!("{}:{}", args.ip, args.port))
        .await
        .unwrap();
    let app = Router::new().route("/{*key}", routing::get(handler));
    let _ = axum::serve(listener, app).await.unwrap();
}

#[derive(Deserialize, Serialize)]
struct Params {
    status: Option<u16>,
    latency_ms: Option<u64>,
}

#[derive(Serialize)]
struct Response {
    path: String,
    status: u16,
    lateny_ms: Option<u64>,
}

async fn handler(
    Path(path): Path<String>,
    Query(params): Query<Params>,
) -> (StatusCode, Json<Response>) {
    let status = params
        .status
        .and_then(|s| StatusCode::from_u16(s).ok())
        .unwrap_or(StatusCode::OK);

    if let Some(delay) = params.latency_ms {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    (
        status,
        Json(Response {
            path,
            status: status.as_u16(),
            lateny_ms: params.latency_ms,
        }),
    )
}
