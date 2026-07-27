//! Pattern detection endpoints (single symbol, multi-scan, optional MTF).

use crate::mtf::MtfEngine;
use crate::pattern::{apply_strategy, detect_setups, SetupSeries, ALL_PATTERNS, SETUP_PATTERNS};
use crate::session::SessionContext;
use crate::state::AppState;
use crate::strategy::{self};
use crate::timeframe::Timeframe;
use crate::universe::name_for;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

fn parse_tf(tf: &str) -> Result<Timeframe, (StatusCode, String)> {
    Timeframe::parse(tf).ok_or((StatusCode::BAD_REQUEST, format!("invalid timeframe: {tf}")))
}

#[derive(Deserialize)]
struct PatternQuery {
    #[serde(default = "d_tf")]
    tf: String,
    #[serde(default = "d_strategy")]
    strategy: String,
    #[serde(default)]
    mtf: bool,
}
fn d_tf() -> String {
    "5m".into()
}
fn d_strategy() -> String {
    "balanced".into()
}

/// `GET /api/patterns/{shcode}` — patterns for one symbol (optionally MTF-scored).
async fn get_patterns(State(st): State<AppState>, Path(shcode): Path<String>, Query(q): Query<PatternQuery>) -> ApiResult {
    let tf = parse_tf(&q.tf)?;
    let cfg = strategy::preset(&q.strategy);
    let token = st.token().await?;
    let candles = st.fetcher.fetch(&token, &shcode, tf).await;
    let mut results = st.detector.scan(&candles, tf, 0.0, true, &cfg, 1);

    if q.mtf && !results.is_empty() {
        let engine = MtfEngine::new(&st.fetcher, &st.detector);
        for r in &mut results {
            r.mtf_score = engine.score(&token, &shcode, tf, &r.pattern_type).await;
            apply_strategy(r, &cfg, true, false, false);
        }
    }
    // Merge context setups (VWAP / ORB / EMA / RSI / BB) for the latest closed bar.
    if candles.len() >= 3 {
        let ctx = SessionContext::for_tf(&candles, tf);
        let series = SetupSeries::compute(&candles);
        let mut setups = detect_setups(&candles, candles.len() - 1, &ctx, &series, &cfg.enabled_patterns, cfg.allows_short());
        for s in &mut setups {
            apply_strategy(s, &cfg, false, false, false);
        }
        results.append(&mut setups);
    }
    results.sort_by(|a, b| b.composite_score.partial_cmp(&a.composite_score).unwrap_or(std::cmp::Ordering::Equal));
    Ok(Json(json!({
        "shcode": shcode, "name": name_for(&shcode), "timeframe": q.tf, "patterns": results,
    })))
}

/// `POST /api/patterns/scan` — scan up to 50 codes `{codes, tf, strategy}`.
async fn scan_many(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let codes: Vec<String> = body
        .get("codes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).take(50).collect())
        .unwrap_or_default();
    let tf = parse_tf(body.get("tf").and_then(Value::as_str).unwrap_or("5m"))?;
    let cfg = strategy::resolve(body.get("strategy").unwrap_or(&json!("balanced")));
    let token = st.token().await?;

    let mut out = vec![];
    for code in codes {
        let candles = st.fetcher.fetch(&token, &code, tf).await;
        let results = st.detector.scan(&candles, tf, 0.0, true, &cfg, 1);
        out.push(json!({"shcode": code, "name": name_for(&code), "patterns": results}));
    }
    Ok(Json(json!(out)))
}

/// `GET /api/patterns/catalog` — all detectable pattern + setup names.
async fn pattern_catalog() -> ApiResult {
    Ok(Json(json!({
        "candlestick": ALL_PATTERNS,
        "setups": SETUP_PATTERNS,
    })))
}

/// Pattern routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/patterns/catalog", get(pattern_catalog))
        .route("/api/patterns/scan", post(scan_many))
        .route("/api/patterns/:shcode", get(get_patterns))
}
