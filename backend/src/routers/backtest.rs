//! Backtest endpoints — watchlist-wide only (single-symbol backtest removed).
//!
//! Two surfaces, both operating over the whole watchlist (`shcodes`):
//!   - `/api/backtest/strategy-matrix` : 트레이더(전략 프리셋) × 종목 검증 + 베스트 전략 판정
//!   - `/api/backtest/batch`           : 패턴별 통계(참고용)

use crate::backtest::{backtest_many, evaluate_strategy_live, run_strategy_backtest, CostModel};
use crate::candle::Candle;
use crate::pattern::ALL_PATTERNS;
use crate::risk::RiskConfig;
use crate::state::AppState;
use crate::strategy::{self, presets, StrategyConfig};
use crate::timeframe::Timeframe;
use crate::universe::name_for;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

fn tf_of(body: &Value, default: &str) -> Timeframe {
    Timeframe::parse(body.get("tf").and_then(Value::as_str).unwrap_or(default)).unwrap_or(Timeframe::D1)
}

/// Build a cost model from an optional `cost` object in the request body.
fn cost_of(body: &Value) -> CostModel {
    let c = body.get("cost").cloned().unwrap_or(json!({}));
    let f = |k: &str, d: f64| c.get(k).and_then(Value::as_f64).unwrap_or(d);
    CostModel { fee_rate: f("fee_rate", 0.00015), tax_rate: f("tax_rate", 0.0015), slippage_rate: f("slippage_rate", 0.0008) }
}

/// Build a risk config (only the stop/target/trailing fields the UI sends).
fn risk_of(body: &Value) -> RiskConfig {
    let r = body.get("risk").cloned().unwrap_or(json!({}));
    let f = |k: &str, d: f64| r.get(k).and_then(Value::as_f64).unwrap_or(d);
    RiskConfig {
        stop_loss_atr_mult: f("stop_loss_atr_mult", 1.5),
        take_profit_atr_mult: f("take_profit_atr_mult", 3.0),
        trailing_stop_atr: f("trailing_stop_atr", 2.0),
        ..Default::default()
    }
}

fn all_patterns() -> Vec<String> {
    ALL_PATTERNS.iter().map(|s| s.to_string()).collect()
}

/// Extract a trimmed, non-empty `shcodes` list from the body.
fn codes_of(body: &Value) -> Vec<String> {
    body.get("shcodes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// Signal-weighted mean of `field` over rows that have OOS signals.
fn weighted(rows: &[&Value], field: &str) -> f64 {
    let total: u64 = rows.iter().map(|r| r["oos_total_signals"].as_u64().unwrap_or(0)).sum();
    if total == 0 {
        return 0.0;
    }
    rows.iter()
        .map(|r| r[field].as_f64().unwrap_or(0.0) * r["oos_total_signals"].as_u64().unwrap_or(0) as f64)
        .sum::<f64>()
        / total as f64
}

/// `POST /api/backtest/strategy-matrix` — every preset (트레이더) against every watchlist
/// symbol, live-equivalent in-sample + walk-forward OOS, plus a per-strategy aggregate so the
/// caller can see which strategy is actually best.
///
/// Timeframe modes (body `tf`):
///   - a concrete value (`"1m"`, `"5m"`, …): every strategy is tested on that timeframe.
///   - `"auto"`: each strategy is tested on **its own recommended timeframe**
///     (reversal presets → 5m, day-trading setups → 1m), so all strategies compete fairly.
/// Candles are fetched once per (symbol, timeframe) and reused across presets.
async fn strategy_matrix(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let codes = codes_of(&body);
    if codes.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "shcodes required (관심종목이 비어 있습니다)".into()));
    }
    let tf_param = body.get("tf").and_then(Value::as_str).unwrap_or("1m").to_string();
    let auto = tf_param == "auto";
    let fixed_tf = tf_of(&body, "1m"); // ignored in auto mode
    let cost = cost_of(&body);
    let risk = risk_of(&body);
    let max_hold = body.get("max_hold_bars").and_then(Value::as_u64).unwrap_or(25) as usize;
    let token = st.token().await?;
    let presets = presets();

    // Timeframe each strategy is evaluated on.
    let tf_for = |cfg: &StrategyConfig| if auto { cfg.recommended_tf() } else { fixed_tf };

    // Per-(strategy, symbol) rows.
    let mut items: Vec<Value> = vec![];
    for code in &codes {
        let name = name_for(code);
        // Fetch candles for each distinct timeframe this run needs (cache dedups).
        let mut by_tf: HashMap<Timeframe, Vec<Candle>> = HashMap::new();
        for cfg in &presets {
            let t = tf_for(cfg);
            if !by_tf.contains_key(&t) {
                by_tf.insert(t, st.fetcher.fetch(&token, code, t).await);
            }
        }
        for cfg in &presets {
            let t = tf_for(cfg);
            let candles = by_tf.get(&t).expect("fetched above");
            if candles.len() < 30 {
                items.push(json!({
                    "strategy": cfg.name, "shcode": code, "name": name, "tf": t.as_str(),
                    "ok": false, "error": "캔들 부족(30개 미만)", "tradeable": false,
                }));
                continue;
            }
            let in_sample = run_strategy_backtest(candles, cfg, t, &risk, &cost, max_hold);
            let oos = evaluate_strategy_live(candles, cfg, t, &risk, &cost);
            items.push(json!({
                "strategy": cfg.name, "shcode": code, "name": name, "tf": t.as_str(), "ok": true, "error": Value::Null,
                "entry_threshold": cfg.entry_threshold,
                "in_sample_signals": in_sample["signals"], "in_sample_avg_return": in_sample["avg_return"],
                "in_sample_win_rate": in_sample["win_rate"], "in_sample_profit_factor": in_sample["profit_factor"],
                "oos_avg_return": oos["oos_avg_return"], "oos_consistency": oos["oos_consistency"],
                "oos_total_signals": oos["oos_total_signals"], "tradeable": oos["tradeable"],
            }));
        }
    }

    // Per-strategy aggregate, ranked best-first (more tradeable symbols, then higher weighted OOS).
    let mut by_strategy: Vec<Value> = vec![];
    for cfg in &presets {
        let rows: Vec<&Value> = items
            .iter()
            .filter(|it| it["strategy"].as_str() == Some(cfg.name.as_str()) && it["ok"].as_bool().unwrap_or(false))
            .collect();
        let graded: Vec<&Value> = rows.iter().filter(|it| it["oos_total_signals"].as_u64().unwrap_or(0) > 0).copied().collect();
        let selected: Vec<&str> = rows
            .iter()
            .filter(|it| it["tradeable"].as_bool().unwrap_or(false))
            .filter_map(|it| it["shcode"].as_str())
            .collect();
        let total_sig: u64 = graded.iter().map(|it| it["oos_total_signals"].as_u64().unwrap_or(0)).sum();
        by_strategy.push(json!({
            "strategy": cfg.name, "entry_threshold": cfg.entry_threshold, "direction": cfg.direction,
            "tf": tf_for(cfg).as_str(),
            "graded_count": graded.len(), "tradeable_count": selected.len(),
            "oos_avg_return": weighted(&graded, "oos_avg_return"),
            "oos_consistency": weighted(&graded, "oos_consistency"),
            "oos_total_signals": total_sig,
            "selected": selected,
        }));
    }
    by_strategy.sort_by(|a, b| {
        let key = |v: &Value| (v["tradeable_count"].as_u64().unwrap_or(0), v["oos_avg_return"].as_f64().unwrap_or(0.0));
        key(b).partial_cmp(&key(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let best = by_strategy.first().and_then(|s| s["strategy"].as_str()).map(String::from);

    Ok(Json(json!({
        "timeframe": if auto { "auto".to_string() } else { fixed_tf.as_str().to_string() },
        "auto": auto, "max_hold_bars": max_hold,
        "round_trip_cost_pct": cost.round_trip_cost() * 100.0,
        "count": codes.len(),
        "strategies": presets.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        "best_strategy": best, "by_strategy": by_strategy, "items": items,
    })))
}

/// `POST /api/backtest/batch` — pattern backtest across the watchlist (reference statistics).
async fn backtest_batch(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let codes = codes_of(&body);
    if codes.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "shcodes required (관심종목이 비어 있습니다)".into()));
    }
    let tf = tf_of(&body, "1d");
    let hold_bars = body.get("hold_bars").and_then(Value::as_u64).unwrap_or(5) as usize;
    let cost = cost_of(&body);
    let strategy_name = body.get("strategy").and_then(Value::as_str);
    let patterns: Vec<String> = match strategy_name {
        Some(name) => strategy::preset(name).enabled_patterns,
        None => body
            .get("pattern_names")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_else(all_patterns),
    };
    let token = st.token().await?;

    let mut items: Vec<Value> = vec![];
    for code in &codes {
        let candles = st.fetcher.fetch(&token, code, tf).await;
        if candles.len() < 20 {
            items.push(json!({"shcode": code, "name": name_for(code), "ok": false, "error": "캔들 부족(20개 미만)",
                              "total_signals": 0, "win_rate": 0.0, "avg_return": 0.0, "by_pattern": []}));
            continue;
        }
        let mut res = backtest_many(&candles, &patterns, tf, hold_bars, &cost);
        res["shcode"] = json!(code);
        res["name"] = json!(name_for(code));
        res["ok"] = json!(true);
        res["error"] = Value::Null;
        items.push(res);
    }
    // Signal-weighted aggregate over graded items.
    let graded: Vec<&Value> = items.iter().filter(|it| it["ok"].as_bool().unwrap_or(false) && it["total_signals"].as_u64().unwrap_or(0) > 0).collect();
    let total_signals: u64 = graded.iter().map(|it| it["total_signals"].as_u64().unwrap_or(0)).sum();
    let (agg_avg, agg_win) = if total_signals > 0 {
        let t = total_signals as f64;
        (
            graded.iter().map(|it| it["avg_return"].as_f64().unwrap_or(0.0) * it["total_signals"].as_u64().unwrap_or(0) as f64).sum::<f64>() / t,
            graded.iter().map(|it| it["win_rate"].as_f64().unwrap_or(0.0) * it["total_signals"].as_u64().unwrap_or(0) as f64).sum::<f64>() / t,
        )
    } else {
        (0.0, 0.0)
    };
    let graded_count = graded.len();
    items.sort_by(|a, b| b["avg_return"].as_f64().unwrap_or(0.0).partial_cmp(&a["avg_return"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    Ok(Json(json!({
        "timeframe": tf.as_str(), "hold_bars": hold_bars, "strategy": strategy_name,
        "round_trip_cost_pct": cost.round_trip_cost() * 100.0, "count": items.len(),
        "graded_count": graded_count,
        "aggregate": {"total_signals": total_signals, "avg_return": agg_avg, "win_rate": agg_win},
        "items": items,
    })))
}

/// Backtest routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/backtest/strategy-matrix", post(strategy_matrix))
        .route("/api/backtest/batch", post(backtest_batch))
}
