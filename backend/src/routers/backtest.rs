//! Backtest endpoints (cost-aware, walk-forward, preset comparison).

use crate::backtest::{backtest_many, evaluate_strategy_live, run_strategy_backtest, CostModel};
use crate::pattern::ALL_PATTERNS;
use crate::risk::RiskConfig;
use crate::state::AppState;
use crate::strategy::{self, presets};
use crate::timeframe::Timeframe;
use crate::universe::name_for;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

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

/// `POST /api/backtest` — pattern-only fixed-hold backtest.
async fn backtest(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let shcode = body.get("shcode").and_then(Value::as_str).ok_or((StatusCode::BAD_REQUEST, "shcode required".into()))?.to_string();
    let tf = tf_of(&body, "1d");
    let hold_bars = body.get("hold_bars").and_then(Value::as_u64).unwrap_or(5) as usize;
    let patterns = body
        .get("pattern_names")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_else(all_patterns);
    let cost = cost_of(&body);
    let token = st.token().await?;
    let candles = st.fetcher.fetch(&token, &shcode, tf).await;
    let mut result = backtest_many(&candles, &patterns, tf, hold_bars, &cost);
    result["shcode"] = json!(shcode);
    result["timeframe"] = json!(tf.as_str());
    result["hold_bars"] = json!(hold_bars);
    result["round_trip_cost_pct"] = json!(cost.round_trip_cost() * 100.0);
    Ok(Json(result))
}

/// `POST /api/backtest/strategy` — single symbol, live-equivalent + OOS.
async fn backtest_strategy(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let shcode = body.get("shcode").and_then(Value::as_str).ok_or((StatusCode::BAD_REQUEST, "shcode required".into()))?.to_string();
    let tf = tf_of(&body, "5m");
    let cfg = strategy::resolve(body.get("strategy").unwrap_or(&json!("balanced")));
    let cost = cost_of(&body);
    let risk = risk_of(&body);
    let max_hold = body.get("max_hold_bars").and_then(Value::as_u64).unwrap_or(25) as usize;
    let token = st.token().await?;
    let candles = st.fetcher.fetch(&token, &shcode, tf).await;
    let in_sample = run_strategy_backtest(&candles, &cfg, tf, &risk, &cost, max_hold);
    let oos = evaluate_strategy_live(&candles, &cfg, tf, &risk, &cost);
    Ok(Json(json!({
        "shcode": shcode, "timeframe": tf.as_str(), "strategy": cfg.name,
        "entry_threshold": cfg.entry_threshold, "round_trip_cost_pct": cost.round_trip_cost() * 100.0,
        "in_sample": in_sample, "out_of_sample": oos, "tradeable": oos["tradeable"],
    })))
}

/// `POST /api/backtest/strategy-batch` — validate many symbols, select `tradeable` ones.
async fn backtest_strategy_batch(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let codes = codes_of(&body);
    let tf = tf_of(&body, "5m");
    let cfg = strategy::resolve(body.get("strategy").unwrap_or(&json!("balanced")));
    let cost = cost_of(&body);
    let risk = risk_of(&body);
    let max_hold = body.get("max_hold_bars").and_then(Value::as_u64).unwrap_or(25) as usize;
    let token = st.token().await?;

    let mut items: Vec<Value> = vec![];
    for code in &codes {
        let candles = st.fetcher.fetch(&token, code, tf).await;
        if candles.len() < 30 {
            items.push(json!({"shcode": code, "name": name_for(code), "ok": false, "error": "캔들 부족(30개 미만)", "tradeable": false}));
            continue;
        }
        let in_sample = run_strategy_backtest(&candles, &cfg, tf, &risk, &cost, max_hold);
        let oos = evaluate_strategy_live(&candles, &cfg, tf, &risk, &cost);
        items.push(json!({
            "shcode": code, "name": name_for(code), "ok": true, "error": Value::Null,
            "in_sample_signals": in_sample["signals"], "in_sample_avg_return": in_sample["avg_return"],
            "in_sample_win_rate": in_sample["win_rate"], "oos_avg_return": oos["oos_avg_return"],
            "oos_consistency": oos["oos_consistency"], "oos_total_signals": oos["oos_total_signals"],
            "tradeable": oos["tradeable"],
        }));
    }
    // tradeable first, then by OOS net expectancy descending.
    items.sort_by(|a, b| {
        let key = |v: &Value| (v["tradeable"].as_bool().unwrap_or(false), v["oos_avg_return"].as_f64().unwrap_or(0.0));
        let (ta, ra) = key(a);
        let (tb, rb) = key(b);
        (tb, rb).partial_cmp(&(ta, ra)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let selected: Vec<&str> = items.iter().filter(|it| it["tradeable"].as_bool().unwrap_or(false)).filter_map(|it| it["shcode"].as_str()).collect();
    Ok(Json(json!({
        "timeframe": tf.as_str(), "strategy": cfg.name, "entry_threshold": cfg.entry_threshold,
        "round_trip_cost_pct": cost.round_trip_cost() * 100.0, "count": items.len(),
        "selected_count": selected.len(), "selected": selected, "items": items,
    })))
}

/// `POST /api/backtest/batch` — pattern backtest across many symbols.
async fn backtest_batch(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let codes = codes_of(&body);
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

/// `POST /api/backtest/compare-strategies` — pattern-only vs as-traded vs OOS per preset.
async fn compare_strategies(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let shcode = body.get("shcode").and_then(Value::as_str).ok_or((StatusCode::BAD_REQUEST, "shcode required".into()))?.to_string();
    let tf = tf_of(&body, "1d");
    let hold_bars = body.get("hold_bars").and_then(Value::as_u64).unwrap_or(5) as usize;
    let token = st.token().await?;
    let candles = st.fetcher.fetch(&token, &shcode, tf).await;
    let (cost, risk) = (CostModel::default(), RiskConfig::default());

    let mut rows: Vec<Value> = vec![];
    for cfg in presets() {
        let reference = backtest_many(&candles, &cfg.enabled_patterns, tf, hold_bars, &cost);
        let traded = run_strategy_backtest(&candles, &cfg, tf, &risk, &cost, 25);
        let ev = evaluate_strategy_live(&candles, &cfg, tf, &risk, &cost);
        rows.push(json!({
            "preset": cfg.name, "entry_threshold": cfg.entry_threshold,
            "total_signals": traded["signals"], "win_rate": traded["win_rate"],
            "avg_return_net": traded["avg_return"], "profit_factor": traded["profit_factor"],
            "oos_avg_return": ev["oos_avg_return"], "oos_consistency": ev["oos_consistency"], "tradeable": ev["tradeable"],
            "ref_pattern_signals": reference["total_signals"], "ref_pattern_avg_return": reference["avg_return"],
        }));
    }
    rows.sort_by(|a, b| b["oos_avg_return"].as_f64().unwrap_or(0.0).partial_cmp(&a["oos_avg_return"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    Ok(Json(json!({"shcode": shcode, "timeframe": tf.as_str(), "results": rows})))
}

/// Extract a trimmed, non-empty `shcodes` list from the body.
fn codes_of(body: &Value) -> Vec<String> {
    body.get("shcodes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// Backtest routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/backtest", post(backtest))
        .route("/api/backtest/strategy", post(backtest_strategy))
        .route("/api/backtest/strategy-batch", post(backtest_strategy_batch))
        .route("/api/backtest/batch", post(backtest_batch))
        .route("/api/backtest/compare-strategies", post(compare_strategies))
}
