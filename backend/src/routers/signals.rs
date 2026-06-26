//! Watchlist / universe signal scan endpoints.

use crate::pattern::PatternResult;
use crate::state::AppState;
use crate::strategy::{self, StrategyConfig};
use crate::timeframe::Timeframe;
use crate::universe::{filter, name_for, Stock};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

type ApiResult = Result<Json<Value>, (StatusCode, String)>;
const MAX_UNIVERSE_SCAN_LIMIT: usize = 250;

#[derive(Deserialize)]
struct SignalsQuery {
    #[serde(default = "d_market")]
    market: String,
    #[serde(default = "d_tf")]
    tf: String,
    #[serde(default = "d_strategy")]
    strategy: String,
    #[serde(default = "d_min_conf")]
    min_conf: f64,
    #[serde(default = "d_limit")]
    limit: usize,
    #[serde(default = "d_scan_limit")]
    scan_limit: usize,
    #[serde(default = "d_recent_bars")]
    recent_bars: usize,
    min_price: Option<f64>,
    max_price: Option<f64>,
}
fn d_market() -> String { "ALL".into() }
fn d_tf() -> String { "5m".into() }
fn d_strategy() -> String { "balanced".into() }
fn d_min_conf() -> f64 { 0.6 }
fn d_limit() -> usize { 30 }
fn d_scan_limit() -> usize { 120 }
fn d_recent_bars() -> usize { 20 }

/// Build one signal row, keeping only patterns that pass the strategy gates.
fn collect_signals(stock: &Stock, patterns: &[PatternResult], cfg: &StrategyConfig, full_row: bool) -> Vec<Value> {
    patterns
        .iter()
        .filter(|p| p.composite_score >= cfg.entry_threshold)
        .filter(|p| !cfg.require_volume_confirm || p.volume_confirmed)
        .map(|p| {
            if full_row {
                json!({
                    "shcode": stock.code,
                    "name": if stock.name.is_empty() { name_for(&stock.code) } else { stock.name.clone() },
                    "market": stock.market, "price": stock.price, "change_pct": stock.change_pct,
                    "pattern_name": p.pattern_name, "pattern_type": p.pattern_type,
                    "confidence": p.confidence, "composite_score": p.composite_score,
                    "atr_normalized": p.atr_normalized, "volume_confirmed": p.volume_confirmed,
                    "detected_at": p.detected_at, "expected_5": p.expected_5,
                })
            } else {
                json!({
                    "shcode": stock.code, "name": name_for(&stock.code),
                    "pattern_name": p.pattern_name, "pattern_type": p.pattern_type,
                    "confidence": p.confidence, "composite_score": p.composite_score,
                    "detected_at": p.detected_at,
                })
            }
        })
        .collect()
}

fn sort_by_score(signals: &mut [Value]) {
    signals.sort_by(|a, b| {
        let sa = a["composite_score"].as_f64().unwrap_or(0.0);
        let sb = b["composite_score"].as_f64().unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// `GET /api/signals` — scan the (price-filtered) universe for fresh signals.
async fn list_signals(State(st): State<AppState>, Query(q): Query<SignalsQuery>) -> ApiResult {
    let tf = Timeframe::parse(&q.tf).ok_or((StatusCode::BAD_REQUEST, format!("invalid timeframe: {}", q.tf)))?;
    let cfg = strategy::preset(&q.strategy);
    let scan_limit = q.scan_limit.clamp(1, MAX_UNIVERSE_SCAN_LIMIT);
    let recent_bars = q.recent_bars.clamp(1, 60);
    let token = st.token().await?;

    let stocks: Vec<Stock> = filter(&q.market, q.min_price, q.max_price, None, None).into_iter().take(scan_limit).collect();
    let mut signals = vec![];
    for stock in &stocks {
        let candles = st.fetcher.fetch(&token, &stock.code, tf).await;
        let patterns = st.detector.scan(&candles, tf, q.min_conf, true, &cfg, recent_bars);
        signals.extend(collect_signals(stock, &patterns, &cfg, true));
    }
    sort_by_score(&mut signals);
    signals.truncate(q.limit);
    Ok(Json(json!(signals)))
}

/// `POST /api/signals/watchlist` — scan an explicit code list `{codes, tf, strategy, min_conf}`.
async fn scan_watchlist(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let codes: Vec<String> = body
        .get("codes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let tf = Timeframe::parse(body.get("tf").and_then(Value::as_str).unwrap_or("5m"))
        .ok_or((StatusCode::BAD_REQUEST, "invalid timeframe".into()))?;
    let cfg = strategy::resolve(body.get("strategy").unwrap_or(&json!("balanced")));
    let min_conf = body.get("min_conf").and_then(Value::as_f64).unwrap_or(0.6);
    let recent_bars = (body.get("recent_bars").and_then(Value::as_u64).unwrap_or(20) as usize).clamp(1, 60);
    let token = st.token().await?;

    let mut signals = vec![];
    for code in codes {
        let candles = st.fetcher.fetch(&token, &code, tf).await;
        let patterns = st.detector.scan(&candles, tf, min_conf, true, &cfg, recent_bars);
        // Reuse collect_signals with a stub stock carrying only the code.
        let stub = Stock { code: code.clone(), name: String::new(), market: String::new(), price: 0.0, prev_close: 0.0, change_pct: 0.0, volume: 0.0 };
        signals.extend(collect_signals(&stub, &patterns, &cfg, false));
    }
    sort_by_score(&mut signals);
    Ok(Json(json!(signals)))
}

/// Signal routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/signals", get(list_signals))
        .route("/api/signals/watchlist", post(scan_watchlist))
}
