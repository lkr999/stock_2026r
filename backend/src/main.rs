//! Caginalp candlestick trading system — Rust backend entrypoint.
//!
//! Wires the eBest client, pattern detector, journal, and trading engine into an
//! axum app exposing the same `/api/...` surface the SvelteKit frontend expects.
//! All market data comes from the eBest REST API only — never synthetic data.

mod backtest;
mod broker;
mod candle;
mod candle_fetcher;
mod config;
mod ebest;
mod engine;
mod indicators;
mod journal;
mod mtf;
mod pattern;
mod risk;
mod routers;
mod state;
mod strategy;
mod timeframe;
mod universe;
mod validation;

use axum::http::{HeaderValue, Method};
use config::Settings;
use state::AppState;
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    // Load `.env` from the project root and `backend/.env` (latter wins).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let _ = dotenvy::from_path(root.join(".env"));
    let _ = dotenvy::from_path(root.join("backend/.env"));
    tracing_subscriber::fmt().with_target(false).init();

    let settings = Settings::load();
    if !settings.ebest_configured() {
        tracing::warn!(
            "eBest 키 미설정 — .env 의 EBEST_APP_KEY/EBEST_APP_SECRET 를 설정하세요. \
             설정 전에는 데이터 조회 시 인증 오류가 반환됩니다 (가짜 데이터는 생성하지 않습니다)."
        );
    }
    let addr = format!("{}:{}", settings.backend_host, settings.backend_port);
    let cors = build_cors(&settings.cors_origins);
    let st = AppState::new(settings);

    // Merge every resource router; all routes are absolute `/api/...` paths.
    let app = axum::Router::new()
        .merge(routers::misc::router())
        .merge(routers::candles::router())
        .merge(routers::patterns::router())
        .merge(routers::signals::router())
        .merge(routers::backtest::router())
        .merge(routers::trading::router())
        .layer(cors)
        .with_state(st);

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    tracing::info!("Caginalp Candlestick Trader listening on http://{addr}");
    axum::serve(listener, app).await.expect("serve");
}

/// CORS allowing the configured origins plus the SvelteKit dev server.
fn build_cors(origins: &str) -> CorsLayer {
    let mut list: Vec<HeaderValue> = origins
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    if let Ok(dev) = "http://localhost:7777".parse() {
        if !list.contains(&dev) {
            list.push(dev);
        }
    }
    CorsLayer::new()
        .allow_origin(list)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(tower_http::cors::Any)
}
