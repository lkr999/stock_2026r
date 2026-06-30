//! OHLCV candle endpoints (candles, quote, candles+indicators).

use crate::indicators::compute_all;
use crate::state::AppState;
use crate::timeframe::Timeframe;
use crate::universe::name_for;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

#[derive(Deserialize)]
struct TfQuery {
    #[serde(default = "default_tf")]
    tf: String,
}
fn default_tf() -> String {
    "5m".into()
}

/// Parse a timeframe string or return HTTP 400.
fn parse_tf(tf: &str) -> Result<Timeframe, (StatusCode, String)> {
    Timeframe::parse(tf).ok_or((StatusCode::BAD_REQUEST, format!("invalid timeframe: {tf}")))
}

/// `GET /api/candles/{shcode}` — raw candles for a timeframe.
async fn get_candles(State(st): State<AppState>, Path(shcode): Path<String>, Query(q): Query<TfQuery>) -> ApiResult {
    let tf = parse_tf(&q.tf)?;
    let token = st.token().await?;
    let candles = st.fetcher.fetch(&token, &shcode, tf).await;
    Ok(Json(json!({
        "shcode": shcode, "name": name_for(&shcode), "timeframe": q.tf,
        "candles": candles, "data_source": "live",
    })))
}

/// `GET /api/candles/{shcode}/quote` — last close + change vs the prior bar.
async fn get_quote(State(st): State<AppState>, Path(shcode): Path<String>, Query(q): Query<TfQuery>) -> ApiResult {
    let tf = parse_tf(&q.tf)?;
    let token = st.token().await?;
    let candles = st.fetcher.fetch(&token, &shcode, tf).await;
    if candles.is_empty() {
        return Err((StatusCode::NOT_FOUND, "no candle data".into()));
    }
    let last = candles.last().unwrap();
    let prev = if candles.len() >= 2 { &candles[candles.len() - 2] } else { last };
    let change = last.close - prev.close;
    let change_pct = if prev.close != 0.0 { change / prev.close * 100.0 } else { 0.0 };
    Ok(Json(json!({
        "shcode": shcode, "name": name_for(&shcode), "price": last.close, "prev_close": prev.close,
        "change": (change * 100.0).round() / 100.0, "change_pct": (change_pct * 100.0).round() / 100.0,
        "date": last.date, "time": last.time, "data_source": "live",
    })))
}

/// `GET /api/candles/{shcode}/indicators` — candles + MA/Bollinger/RSI/MACD.
async fn get_indicators(State(st): State<AppState>, Path(shcode): Path<String>, Query(q): Query<TfQuery>) -> ApiResult {
    let tf = parse_tf(&q.tf)?;
    let token = st.token().await?;
    let candles = st.fetcher.fetch(&token, &shcode, tf).await;
    let indicators = compute_all(&candles, tf);
    Ok(Json(json!({
        "shcode": shcode, "name": name_for(&shcode), "timeframe": q.tf,
        "candles": candles, "indicators": indicators, "data_source": "live",
    })))
}

/// Candle routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/candles/:shcode", get(get_candles))
        .route("/api/candles/:shcode/quote", get(get_quote))
        .route("/api/candles/:shcode/indicators", get(get_indicators))
}
