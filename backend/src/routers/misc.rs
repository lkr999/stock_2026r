//! Top-level endpoints: health, eBest status/test, universe lookup.

use crate::ebest::parse_float;
use crate::state::AppState;
use crate::timeframe::Timeframe;
use crate::universe::{filter, info_for, name_for};
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Instant;

type Api = Json<Value>;

/// `GET /api/health` — liveness + whether eBest keys are configured.
async fn health(State(st): State<AppState>) -> Api {
    Json(json!({"ok": true, "ebest_configured": st.settings.ebest_configured()}))
}

/// `GET /api/ebest/status` — key presence + whether a token can be issued.
async fn ebest_status(State(st): State<AppState>) -> Api {
    let token_ok = st.ebest.auth_token(false).await.map(|t| !t.is_empty()).unwrap_or(false);
    Json(json!({
        "has_keys": st.settings.ebest_configured(),
        "token_ok": token_ok,
        "base_url": st.ebest.base_url(),
    }))
}

#[derive(Deserialize)]
struct CodeQuery {
    #[serde(default = "d_samsung")]
    code: String,
}
fn d_samsung() -> String {
    "005930".into()
}

/// `POST /api/ebest/test` — step-by-step connectivity check (token → candles → quote).
async fn ebest_test(State(st): State<AppState>, Query(q): Query<CodeQuery>) -> Api {
    let code = q.code;
    let mut checks: Vec<Value> = vec![];

    // 1) Issue an auth token once and reuse it for the data checks.
    let t0 = Instant::now();
    let token = st.ebest.auth_token(false).await.unwrap_or_default();
    let ms = |t: Instant| (t.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0;
    let token_ok = !token.is_empty();
    checks.push(json!({
        "name": "인증 토큰 발급", "tr": "oauth2/token", "ok": token_ok, "latency_ms": ms(t0),
        "detail": if token_ok { format!("발급 성공 (…{})", &token[token.len().saturating_sub(6)..]) } else { "토큰 발급 실패 — eBest 키/네트워크 확인 필요".into() },
    }));

    // 2) 5-minute candles (t8452).
    let t1 = Instant::now();
    let m5 = st.fetcher.fetch(&token, &code, Timeframe::M5).await;
    checks.push(match m5.last() {
        Some(last) => json!({"name": "분봉 조회 (t8452 · 5분)", "tr": "t8452", "ok": true, "latency_ms": ms(t1),
            "detail": format!("{}봉 · 최근 종가 {:.0} ({} {})", m5.len(), last.close, last.date, last.time)}),
        None => json!({"name": "분봉 조회 (t8452 · 5분)", "tr": "t8452", "ok": false, "latency_ms": ms(t1), "detail": "응답 없음"}),
    });

    // 3) Daily candles (t8451).
    let t2 = Instant::now();
    let d1 = st.fetcher.fetch(&token, &code, Timeframe::D1).await;
    checks.push(match d1.last() {
        Some(last) => json!({"name": "일봉 조회 (t8451)", "tr": "t8451", "ok": true, "latency_ms": ms(t2),
            "detail": format!("{}봉 · 최근 종가 {:.0} ({})", d1.len(), last.close, last.date)}),
        None => json!({"name": "일봉 조회 (t8451)", "tr": "t8451", "ok": false, "latency_ms": ms(t2), "detail": "응답 없음"}),
    });

    // 4) Current-price snapshot (t1101).
    let t3 = Instant::now();
    let snap = st.ebest.stock_price(&token, &code).await;
    let price = snap.get("price").and_then(parse_float).unwrap_or(0.0);
    checks.push(if price > 0.0 {
        let drate = snap.get("drate").and_then(Value::as_f64).unwrap_or(0.0);
        json!({"name": "현재가 조회 (t1101)", "tr": "t1101", "ok": true, "latency_ms": ms(t3),
            "detail": format!("현재가 {price:.0}원 (등락 {drate:.2}%)")})
    } else {
        json!({"name": "현재가 조회 (t1101)", "tr": "t1101", "ok": false, "latency_ms": ms(t3), "detail": "현재가 없음"})
    });

    let ok_all = checks.iter().all(|c| c["ok"].as_bool().unwrap_or(false));
    Json(json!({"ok": ok_all, "source": "ebest", "code": code, "name": name_for(&code), "checks": checks}))
}

#[derive(Deserialize)]
struct UniverseQuery {
    #[serde(default = "d_all")]
    market: String,
    min_price: Option<f64>,
    max_price: Option<f64>,
    q: Option<String>,
    #[serde(default = "d_limit")]
    limit: usize,
}
fn d_all() -> String {
    "ALL".into()
}
fn d_limit() -> usize {
    200
}

/// `GET /api/universe` — filtered stock list (market/price/text).
async fn universe(Query(q): Query<UniverseQuery>) -> Api {
    let mut rows = filter(&q.market, q.min_price, q.max_price, None, None);
    if let Some(query) = q.q.as_ref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
        rows.retain(|r| r.name.to_lowercase().contains(&query) || r.code.contains(&query));
    }
    let total = rows.len();
    rows.truncate(q.limit);
    Json(json!({"total": total, "items": rows}))
}

#[derive(Deserialize)]
struct LookupQuery {
    #[serde(default)]
    codes: String,
}

/// `GET /api/universe/lookup` — code → {name, market, price} for comma-separated codes.
async fn lookup(Query(q): Query<LookupQuery>) -> Api {
    let out: Vec<Value> = q
        .codes
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|code| {
            let info = info_for(code);
            json!({
                "code": code,
                "name": info.map(|s| s.name.clone()).unwrap_or_default(),
                "market": info.map(|s| s.market.clone()).unwrap_or_default(),
                "price": info.map(|s| s.price).unwrap_or(0.0),
            })
        })
        .collect();
    Json(json!(out))
}

/// `GET /api/universe/price-range` — min/max current price for a market.
async fn price_range(Query(q): Query<UniverseQuery>) -> Api {
    let rows = filter(&q.market, None, None, None, None);
    let prices: Vec<f64> = rows.iter().map(|r| r.price).filter(|&p| p > 0.0).collect();
    let min = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    Json(json!({
        "count": rows.len(),
        "min": if prices.is_empty() { 0.0 } else { min },
        "max": prices.iter().cloned().fold(0.0, f64::max),
    }))
}

/// Top-level routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/ebest/status", get(ebest_status))
        .route("/api/ebest/test", post(ebest_test))
        .route("/api/universe", get(universe))
        .route("/api/universe/lookup", get(lookup))
        .route("/api/universe/price-range", get(price_range))
}
