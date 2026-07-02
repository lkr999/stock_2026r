//! Automated trading control endpoints (spec section 10-5).

use crate::backtest::{evaluate_strategy_live, CostModel};
use crate::broker::{Broker, TradingMode};
use crate::config::Settings;
use crate::engine::{OrderConfig, TradingEngine};
use crate::risk::{RiskConfig, RiskManager};
use crate::state::AppState;
use crate::strategy::{self, presets, Source, StrategyConfig};
use crate::timeframe::Timeframe;
use crate::validation::{evaluate, ReadinessCriteria};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

/// Parse per-symbol strategy overrides from the request body.
/// `{ "005930": "vwap_scalp", "000660": { "name": "balanced", ... } }`
fn symbol_strategies_of(body: &Value) -> HashMap<String, StrategyConfig> {
    body.get("symbol_strategies")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(code, v)| (code.clone(), strategy::resolve(v))).collect())
        .unwrap_or_default()
}

/// Readiness thresholds from settings.
fn criteria(s: &Settings) -> ReadinessCriteria {
    ReadinessCriteria {
        min_paper_trades: s.live_min_paper_trades,
        min_paper_days: s.live_min_paper_days,
        min_win_rate: s.live_min_win_rate,
        min_profit_factor: s.live_min_profit_factor,
        require_positive_pnl: s.live_require_positive_pnl,
    }
}

#[derive(Deserialize)]
struct ModeQuery {
    #[serde(default = "d_all")]
    mode: String,
}
fn d_all() -> String { "all".into() }

#[derive(Deserialize)]
struct JournalQuery {
    #[serde(default = "d_paper")]
    mode: String,
    #[serde(default = "d_journal_limit")]
    limit: usize,
}
fn d_paper() -> String { "paper".into() }
fn d_journal_limit() -> usize { 100 }

/// `GET /api/trading/readiness` — paper record vs the live-gate criteria (advisory).
async fn readiness(State(st): State<AppState>) -> ApiResult {
    let mut report = evaluate(&st.journal, &criteria(&st.settings));
    report["force_allowed"] = json!(st.settings.live_allow_force);
    Ok(Json(report))
}

/// `GET /api/trading/balance` — real eBest account deposit (t0424), for the
/// live-mode "매수 한도액" default (총 진입금액 한도 = 잔고금액).
async fn balance(State(st): State<AppState>) -> ApiResult {
    let token = st.token().await?;
    let bal = st.ebest.get_account_balance(&token).await;
    let deposit = bal
        .get("t0424OutBlock")
        .and_then(|b| b.get("sunamt"))
        .and_then(crate::ebest::parse_float)
        .unwrap_or(0.0);
    Ok(Json(json!({"balance": deposit})))
}

/// `GET /api/trading/journal` — recent trades for a mode, newest first.
async fn journal(State(st): State<AppState>, Query(q): Query<JournalQuery>) -> ApiResult {
    let mut trades = st.journal.by_mode(&q.mode);
    let start = trades.len().saturating_sub(q.limit);
    let mut recent: Vec<Value> = trades.drain(start..).collect();
    recent.reverse();
    Ok(Json(json!(recent)))
}

/// `GET /api/trading/stats` — aggregate trade statistics.
async fn stats(State(st): State<AppState>, Query(q): Query<ModeQuery>) -> ApiResult {
    let mode = if q.mode == "all" { None } else { Some(q.mode.as_str()) };
    Ok(Json(st.journal.stats(mode)))
}

/// `POST /api/trading/positions/{code}/close` — manually close one position.
async fn close_position(State(st): State<AppState>, Path(code): Path<String>) -> ApiResult {
    let engine = st.engine.lock().await.clone();
    let Some(engine) = engine else {
        return Err((StatusCode::NOT_FOUND, "no engine".into()));
    };
    let res = engine.manual_close(&code).await;
    if !res["ok"].as_bool().unwrap_or(false) {
        let reason = res["reason"].as_str().unwrap_or("close_failed");
        let code = if reason == "no_position" { StatusCode::NOT_FOUND } else { StatusCode::BAD_REQUEST };
        return Err((code, reason.into()));
    }
    let mut out = res;
    out["status"] = engine.status().await;
    Ok(Json(out))
}

/// `POST /api/trading/events/clear` — clear this session's in-memory events.
async fn clear_events(State(st): State<AppState>) -> ApiResult {
    let engine = st.engine.lock().await.clone();
    match engine {
        Some(e) => {
            let removed = e.clear_events().await;
            Ok(Json(json!({"ok": true, "removed": removed, "status": e.status().await})))
        }
        None => Ok(Json(json!({"ok": true, "removed": 0,
            "status": {"running": false, "positions": [], "trade_events": []}}))),
    }
}

/// `POST /api/trading/journal/clear` — erase persisted trades (all/paper/live).
async fn clear_journal(State(st): State<AppState>, Query(q): Query<ModeQuery>) -> ApiResult {
    let removed = if q.mode == "all" {
        let before = st.journal.all().len();
        st.journal.clear();
        before
    } else {
        st.journal.clear_mode(&q.mode)
    };
    Ok(Json(json!({"ok": true, "removed": removed})))
}

/// `GET /api/trading/presets` — the built-in strategy presets.
async fn list_presets() -> ApiResult {
    let mut out = serde_json::Map::new();
    for cfg in presets() {
        let weights: serde_json::Map<String, Value> = Source::all()
            .iter()
            .map(|s| (s.as_str().to_string(), json!(cfg.weights.get(s).copied().unwrap_or(0.0))))
            .collect();
        out.insert(cfg.name.clone(), json!({
            "name": cfg.name, "weights": weights, "enabled_patterns": cfg.enabled_patterns,
            "entry_threshold": cfg.entry_threshold, "direction": cfg.direction,
            "recommended_tf": cfg.recommended_tf,
        }));
    }
    Ok(Json(Value::Object(out)))
}

/// Build a full `RiskConfig` from the request body's `risk` object.
fn risk_config(body: &Value, settings: &Settings) -> RiskConfig {
    let r = body.get("risk").cloned().unwrap_or(json!({}));
    let f = |k: &str, d: f64| r.get(k).and_then(Value::as_f64).unwrap_or(d);
    let i = |k: &str, d: i64| r.get(k).and_then(Value::as_i64).unwrap_or(d);
    let b = |k: &str, d: bool| r.get(k).and_then(Value::as_bool).unwrap_or(d);
    RiskConfig {
        max_position_pct: f("max_position_pct", 0.10),
        max_positions: i("max_positions", settings.trading_max_positions as i64) as usize,
        risk_per_trade_pct: f("risk_per_trade_pct", settings.trading_risk_per_trade),
        stop_loss_atr_mult: f("stop_loss_atr_mult", 1.5),
        take_profit_atr_mult: f("take_profit_atr_mult", 3.0),
        stop_loss_pct: f("stop_loss_pct", 0.0),
        take_profit_pct: f("take_profit_pct", 0.0),
        daily_loss_limit_pct: f("daily_loss_limit_pct", settings.trading_daily_loss_limit),
        trailing_stop_atr: f("trailing_stop_atr", 2.0),
        reentry_cooldown_bars: i("reentry_cooldown_bars", 1),
        loss_cooldown_bars: i("loss_cooldown_bars", 3),
        reentry_gap_pct: f("reentry_gap_pct", 0.0),
        fib_averaging_enabled: b("fib_averaging_enabled", false),
        fib_max_levels: i("fib_max_levels", 0),
        require_confirmation: b("require_confirmation", true),
        require_higher_tf_uptrend: b("require_higher_tf_uptrend", true),
        min_hold_bars: i("min_hold_bars", 1),
        hard_stop_intrabar: b("hard_stop_intrabar", false),
        hard_stop_buffer_pct: f("hard_stop_buffer_pct", 0.0),
    }
}

/// Build an `OrderConfig` from the request body's `order` object.
fn order_config(body: &Value, settings: &Settings) -> OrderConfig {
    let o = body.get("order").cloned().unwrap_or(json!({}));
    let fixed = o.get("fixed_qty").and_then(Value::as_i64).unwrap_or(settings.trading_fixed_qty);
    OrderConfig {
        order_type: o.get("order_type").and_then(Value::as_str).unwrap_or(&settings.trading_order_type).to_string(),
        fixed_qty: (fixed != 0).then_some(fixed),
        sell_all: o.get("sell_all").and_then(Value::as_bool).unwrap_or(settings.trading_sell_all),
        max_buy_amount: o.get("max_buy_amount").and_then(Value::as_f64).unwrap_or(settings.trading_max_buy_amount),
    }
}

/// Keep only OOS-validated (`tradeable`) codes; return (kept, dropped).
///
/// Each symbol is validated with the **same strategy + timeframe it will actually trade on**:
/// the per-symbol assignment (from the dashboard backtest) when present, else the global
/// strategy/TF. This keeps the OOS gate consistent with the engine's `tf_for`/`strategy_for`.
async fn filter_tradeable(
    st: &AppState,
    cfg: &StrategyConfig,
    symbol_strats: &HashMap<String, StrategyConfig>,
    watchlist: &[String],
    tf: Timeframe,
    risk: &RiskConfig,
) -> (Vec<String>, Vec<String>) {
    let token = st.ebest.auth_token(false).await.unwrap_or_default();
    let cost = CostModel::default();
    let (mut kept, mut dropped) = (vec![], vec![]);
    for code in watchlist {
        let sym_cfg = symbol_strats.get(code).unwrap_or(cfg);
        let sym_tf = symbol_strats.get(code).map(|c| c.recommended_tf()).unwrap_or(tf);
        let candles = st.fetcher.fetch(&token, code, sym_tf).await;
        let oos = evaluate_strategy_live(&candles, sym_cfg, sym_tf, risk, &cost);
        if oos["tradeable"].as_bool().unwrap_or(false) {
            kept.push(code.clone());
        } else {
            dropped.push(code.clone());
        }
    }
    (kept, dropped)
}

/// `POST /api/trading/start` — start or resume the engine (mode is user-chosen).
async fn start(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let settings = &st.settings;
    let mode = TradingMode::parse(body.get("mode").and_then(Value::as_str).unwrap_or(&settings.trading_mode));
    let readiness_report = evaluate(&st.journal, &criteria(settings));

    let cfg = strategy::resolve(body.get("strategy").unwrap_or(&json!(settings.trading_default_strategy)));
    let symbol_strats = symbol_strategies_of(&body);
    let mut watchlist: Vec<String> = body
        .get("watchlist")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if watchlist.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "watchlist required".into()));
    }
    let tf = Timeframe::parse(body.get("tf").and_then(Value::as_str).unwrap_or("1m")).unwrap_or(Timeframe::M1);
    let poll_sec = body.get("poll_sec").and_then(Value::as_u64).unwrap_or(60);
    let risk_cfg = risk_config(&body, settings);

    // Optional OOS pre-filter of the watchlist — validated with the same risk
    // settings the engine will actually trade with (not the defaults).
    let mut dropped: Vec<String> = vec![];
    if body.get("require_tradeable").and_then(Value::as_bool).unwrap_or(false) {
        let (kept, drop) = filter_tradeable(&st, &cfg, &symbol_strats, &watchlist, tf, &risk_cfg).await;
        watchlist = kept;
        dropped = drop;
        if watchlist.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "OOS 검증을 통과한 종목이 없습니다. 전략/타임프레임을 바꾸거나 require_tradeable 를 끄세요.".into()));
        }
    }

    let mut order_cfg = order_config(&body, settings);
    // 현재 주문 경로는 "접수 성공 = 요청가 체결"로 간주한다. 지정가가 미체결로 남으면
    // 내부 포지션/현금이 실계좌와 어긋나므로, 체결 대사가 구현되기 전까지 실전은
    // 시장가만 허용한다 (모의투자는 즉시 체결 시뮬레이션이라 무관).
    if mode == TradingMode::Live && order_cfg.order_type != "market" {
        tracing::warn!("live mode: order_type '{}' → 'market' 로 강제 (미체결 대사 미구현)", order_cfg.order_type);
        order_cfg.order_type = "market".into();
    }
    let ignore_hours = body.get("ignore_market_hours").and_then(Value::as_bool).unwrap_or(false);
    let reset = body.get("reset").and_then(Value::as_bool).unwrap_or(false);

    // Hold the engine slot lock across check + reuse/create so two concurrent
    // start requests cannot each spawn an engine (one of which would become an
    // orphaned trading loop with no handle).
    let mut guard = st.engine.lock().await;

    // Already running? Apply the new config in place (keep positions/cash) instead of
    // refusing — this is how a watchlist change takes effect: held codes stay, the rest
    // are replaced by the updated watchlist on the next cycle.
    if let Some(engine) = guard.clone() {
        if engine.running().await {
            if reset {
                return Err((StatusCode::CONFLICT, "리셋하려면 먼저 자동매매를 중지하세요.".into()));
            }
            if engine.mode() != mode {
                return Err((StatusCode::CONFLICT,
                    format!("이미 {} 모드로 실행 중입니다. 모드를 바꾸려면 먼저 중지하세요.", engine.mode().as_str())));
            }
            engine
                .reconfigure(cfg, symbol_strats, watchlist, tf, ignore_hours, order_cfg, risk_cfg, settings.trading_paper_seed)
                .await;
            let status = engine.status().await;
            return Ok(Json(json!({
                "ok": true, "mode": mode.as_str(), "resumed": true, "reconfigured": true,
                "dropped_untradeable": dropped, "status": status,
                "readiness_advisory": Value::Null,
            })));
        }
    }

    // Reuse the existing engine (keep positions/cash) only when same mode + not resetting.
    let reuse = matches!(&*guard, Some(e) if !reset && e.mode() == mode);
    let engine: Arc<TradingEngine>;
    if reuse {
        engine = guard.clone().unwrap();
        engine
            .reconfigure(cfg, symbol_strats, watchlist, tf, ignore_hours, order_cfg, risk_cfg, settings.trading_paper_seed)
            .await;
        engine.start(poll_sec, true).await;
    } else {
        // Make sure any previous engine's loop is stopped before dropping our
        // only reference to it — otherwise its spawned task would keep trading.
        if let Some(old) = guard.take() {
            old.stop().await;
        }
        let broker = Broker::new(Some(st.ebest.clone()), mode);
        let new_engine = Arc::new(TradingEngine::new(
            Some(st.ebest.clone()),
            st.fetcher.clone(),
            st.detector.clone(),
            broker,
            cfg,
            symbol_strats,
            RiskManager::new(risk_cfg),
            watchlist,
            tf,
            ignore_hours,
            st.journal.clone(),
            settings.trading_paper_seed,
            order_cfg,
        ));
        new_engine.start(poll_sec, false).await;
        *guard = Some(new_engine.clone());
        engine = new_engine;
    }
    drop(guard);

    // Advisory only — never blocks live trading.
    let advisory = if mode != TradingMode::Live || readiness_report["ready"].as_bool().unwrap_or(false) {
        Value::Null
    } else {
        let failed: Vec<&str> = readiness_report["criteria"]
            .as_array()
            .map(|cs| cs.iter().filter(|c| !c["passed"].as_bool().unwrap_or(false)).filter_map(|c| c["label"].as_str()).collect())
            .unwrap_or_default();
        json!({"recommended": false, "message": "검증 기준 미달 상태에서 실전투자를 시작했습니다. 권장: 모의투자로 충분히 검증하세요.", "failed_criteria": failed})
    };
    Ok(Json(json!({
        "ok": true, "mode": mode.as_str(), "resumed": reuse, "dropped_untradeable": dropped,
        "status": engine.status().await, "readiness_advisory": advisory,
    })))
}

/// `POST /api/trading/stop` — stop the engine.
async fn stop(State(st): State<AppState>) -> ApiResult {
    let engine = st.engine.lock().await.clone();
    let Some(engine) = engine else {
        return Err((StatusCode::NOT_FOUND, "no engine".into()));
    };
    engine.stop().await;
    Ok(Json(json!({"ok": true, "status": engine.status().await})))
}

/// `GET /api/trading/status` — engine status (or an idle stub).
async fn status(State(st): State<AppState>) -> ApiResult {
    let engine = st.engine.lock().await.clone();
    match engine {
        Some(e) => Ok(Json(e.status().await)),
        None => Ok(Json(json!({"running": false, "positions": [], "daily_pnl": 0.0}))),
    }
}

/// `PUT /api/trading/strategy` — swap the live strategy weights.
async fn update_strategy(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let engine = st.engine.lock().await.clone();
    let Some(engine) = engine else {
        return Err((StatusCode::NOT_FOUND, "no engine".into()));
    };
    let name = engine.set_strategy(strategy::resolve(&body)).await;
    Ok(Json(json!({"ok": true, "strategy": name})))
}

/// Trading routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trading/readiness", get(readiness))
        .route("/api/trading/balance", get(balance))
        .route("/api/trading/journal", get(journal))
        .route("/api/trading/journal/clear", post(clear_journal))
        .route("/api/trading/stats", get(stats))
        .route("/api/trading/positions/:code/close", post(close_position))
        .route("/api/trading/events/clear", post(clear_events))
        .route("/api/trading/presets", get(list_presets))
        .route("/api/trading/start", post(start))
        .route("/api/trading/stop", post(stop))
        .route("/api/trading/status", get(status))
        .route("/api/trading/strategy", put(update_strategy))
}
