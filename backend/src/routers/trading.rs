//! Automated trading control endpoints (spec section 10-5).

use crate::backtest::{evaluate_strategy_live, CostModel, MtfContext};
use crate::broker::{Broker, TradingMode};
use crate::config::Settings;
use crate::engine::{OrderConfig, TradingEngine};
use crate::risk::{RiskConfig, RiskManager};
use crate::state::AppState;
use crate::strategy::{self, StrategyConfig};
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

/// `GET /api/trading/presets` — the built-in strategy presets of both
/// generations, plus a `_index` listing each generation's members so the picker
/// can keep legacy and v2 strictly apart.
async fn list_presets() -> ApiResult {
    let mut out = serde_json::Map::new();
    for cfg in strategy::all_presets() {
        out.insert(cfg.name.clone(), cfg.to_json());
    }
    let names = |gen| {
        strategy::presets_of(gen)
            .into_iter()
            .map(|c| c.name)
            .collect::<Vec<_>>()
    };
    out.insert(
        "_index".into(),
        json!({
            "legacy": {
                "label": "기존 전략 (레거시)",
                "note": "진입 신호만 정의합니다. 손절·익절은 아래 폼 값을 그대로 사용하며, 시장(지수) 필터가 없습니다.",
                "names": names(strategy::Generation::Legacy),
            },
            "v2": {
                "label": "신형 전략 (V2)",
                "note": "진입 신호 + 전용 리스크 규칙 + 시장 필터를 함께 정의합니다. 손절·익절은 전략이 강제하므로 폼의 고정% 설정을 덮어씁니다.",
                "names": names(strategy::Generation::V2),
            },
        }),
    );
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
        use_stop_loss: b("use_stop_loss", true),
        use_take_profit: b("use_take_profit", true),
        use_trailing_stop: b("use_trailing_stop", true),
        reentry_cooldown_bars: i("reentry_cooldown_bars", 1),
        loss_cooldown_bars: i("loss_cooldown_bars", 3),
        reentry_gap_pct: f("reentry_gap_pct", 0.0),
        reentry_guard_expire_bars: i("reentry_guard_expire_bars", 20),
        fib_averaging_enabled: b("fib_averaging_enabled", false),
        // 물타기 차수 상한 5차 — 피보나치 수량(1·1·2·3·5)이 그 이상부터는
        // 기하급수적으로 커져 하락장에서 계좌를 위협한다 (안전장치).
        fib_max_levels: i("fib_max_levels", 0).clamp(0, 5),
        require_confirmation: b("require_confirmation", true),
        confirm_window_bars: i("confirm_window_bars", 3),
        require_higher_tf_uptrend: b("require_higher_tf_uptrend", true),
        higher_tf_slope_tolerance: f("higher_tf_slope_tolerance", 0.0),
        min_hold_bars: i("min_hold_bars", 1),
        hard_stop_intrabar: b("hard_stop_intrabar", false),
        hard_stop_buffer_pct: f("hard_stop_buffer_pct", 0.0),
        eod_flatten: b("eod_flatten", true),
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
    max_hold_bars: usize,
) -> Result<(Vec<String>, Vec<String>), (StatusCode, String)> {
    // An auth failure must be surfaced, not swallowed: with an empty token every
    // fetch returns no candles, every symbol fails OOS, and the user sees a
    // misleading "no symbol passed validation" error.
    let token = st.token().await.map_err(|(code, msg)| {
        (code, format!("OOS 선별을 수행할 수 없습니다 — {msg}"))
    })?;
    let cost = CostModel::default();
    let (mut kept, mut dropped) = (vec![], vec![]);
    for code in watchlist {
        let sym_cfg = symbol_strats.get(code).unwrap_or(cfg);
        let sym_tf = symbol_strats.get(code).map(|c| c.recommended_tf()).unwrap_or(tf);
        let candles = st.fetcher.fetch(&token, code, sym_tf).await;
        let mtf = MtfContext::build(&candles, sym_tf, risk.higher_tf_slope_tolerance);
        let oos = evaluate_strategy_live(&candles, sym_cfg, sym_tf, risk, &cost, max_hold_bars, &mtf);
        if oos["tradeable"].as_bool().unwrap_or(false) {
            kept.push(code.clone());
        } else {
            dropped.push(code.clone());
        }
    }
    Ok((kept, dropped))
}

/// 부팅 자동 재개용으로 저장하는 "마지막 시작 요청" 파일 경로.
fn autostart_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/engine_autostart.json")
}

/// 엔진 시작 성공 시 시작 요청 본문을 저장한다. 백엔드가 재시작되면 이 파일로
/// 같은 설정의 엔진을 자동 재개해, 스냅샷에 남은 포지션이 관리 공백(손절/EOD
/// 청산 누락) 없이 이어지도록 한다. 재현 가능하도록 일회성 키는 제거하고
/// watchlist 는 (OOS 필터 통과 후의) 최종 목록으로 교체한다.
fn save_autostart(mode: TradingMode, body: &Value, watchlist: &[String]) {
    let mut b = body.clone();
    if let Some(o) = b.as_object_mut() {
        o.remove("reset");
        o.remove("confirm_discard");
        o.remove("require_tradeable");
        o.insert("watchlist".into(), json!(watchlist));
        o.insert("mode".into(), json!(mode.as_str()));
    }
    let v = json!({
        "enabled": true,
        "mode": mode.as_str(),
        "saved_at": chrono::Utc::now().to_rfc3339(),
        "body": b,
    });
    let path = autostart_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, v.to_string()) {
        tracing::warn!("[autostart] 설정 저장 실패: {e}");
    }
}

/// 자동 재개 플래그만 갱신한다. 사용자가 명시적으로 정지(`/trading/stop`,
/// 텔레그램 /stop)하면 false — 재부팅 시 되살리지 않는다. 시작/재개하면 true.
pub fn set_autostart_enabled(enabled: bool) {
    let path = autostart_path();
    let Ok(raw) = std::fs::read_to_string(&path) else { return };
    let Ok(mut v) = serde_json::from_str::<Value>(&raw) else { return };
    v["enabled"] = json!(enabled);
    let _ = std::fs::write(&path, v.to_string());
}

/// 백엔드 부팅 시 호출: 저장된 시작 설정이 있고 사용자가 정지하지 않은 상태라면
/// 엔진을 자동 재개한다 (스냅샷 복원 → 이월 포지션은 첫 사이클에 즉시 청산).
pub async fn autostart_from_disk(st: AppState) {
    let Ok(raw) = std::fs::read_to_string(autostart_path()) else { return };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        tracing::warn!("[autostart] engine_autostart.json 파싱 실패 — 자동 재개 생략");
        return;
    };
    if !v.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        tracing::info!("[autostart] 이전 세션에서 명시적으로 정지됨 — 자동 재개 안 함");
        return;
    }
    let Some(body) = v.get("body").cloned() else { return };
    let mode = v.get("mode").and_then(Value::as_str).unwrap_or("?").to_string();
    tracing::info!("[autostart] 백엔드 재시작 감지 — {mode} 엔진 자동 재개 시도");
    match start_engine(st, body).await {
        Ok(_) => tracing::info!("[autostart] {mode} 엔진 자동 재개 완료 (스냅샷 포지션 복원 포함)"),
        Err((code, msg)) => tracing::warn!("[autostart] 자동 재개 실패 ({code}): {msg}"),
    }
}

/// `POST /api/trading/start` — start or resume the engine (mode is user-chosen).
async fn start(State(st): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    start_engine(st, body).await
}

/// Engine start/resume core — shared by the HTTP handler and boot autostart.
pub async fn start_engine(st: AppState, body: Value) -> ApiResult {
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
        // OOS 선별도 백테스트 화면과 같은 보유봉수 기준으로 판정한다 (기본 25).
        let max_hold = body.get("max_hold_bars").and_then(Value::as_u64).unwrap_or(25) as usize;
        let (kept, drop) = filter_tradeable(&st, &cfg, &symbol_strats, &watchlist, tf, &risk_cfg, max_hold).await?;
        watchlist = kept;
        dropped = drop;
        if watchlist.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "OOS 검증을 통과한 종목이 없습니다. 전략/타임프레임을 바꾸거나 require_tradeable 를 끄세요.".into()));
        }
    }

    let order_cfg = order_config(&body, settings);
    // 지정가/최유리 실전 주문은 브로커의 체결 대사(t0425 폴링 → 잔량 취소)가
    // 실제 체결수량·평균체결가를 확인하므로 시장가 강제가 더 이상 필요 없다.
    let mut ignore_hours = body.get("ignore_market_hours").and_then(Value::as_bool).unwrap_or(false);
    // 실전에서 장시간 무시는 스테일 봉 기반 실주문 + EOD 청산 무력화로 이어진다 — 항상 강제 해제.
    if mode == TradingMode::Live && ignore_hours {
        tracing::warn!("live mode: ignore_market_hours → false 로 강제");
        ignore_hours = false;
    }
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
                .reconfigure(cfg, symbol_strats, watchlist.clone(), tf, ignore_hours, order_cfg, risk_cfg, settings.trading_paper_seed)
                .await;
            save_autostart(mode, &body, &watchlist);
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

    // Discarding a previous engine (mode switch or reset) drops its open
    // positions from tracking. That must never happen silently — require an
    // explicit `confirm_discard` from the caller when positions are held.
    if !reuse {
        if let Some(old) = &*guard {
            let old_status = old.status().await;
            let held: Vec<String> = old_status["positions"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p["code"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let confirmed = body.get("confirm_discard").and_then(Value::as_bool).unwrap_or(false);
            if !held.is_empty() && !confirmed {
                return Err((StatusCode::CONFLICT, format!(
                    "DISCARD_CONFIRM:이전 {} 엔진에 보유 포지션 {}건({})이 있습니다. \
                     새로 시작하면 이 포지션들의 자동 관리(손절/익절)가 중단됩니다.",
                    old.mode().as_str(), held.len(), held.join(", ")
                )));
            }
        }
    }

    // 이 지점 이후는 실패 경로가 없다 — 시작이 확정되므로 자동 재개 설정을 저장.
    save_autostart(mode, &body, &watchlist);

    let engine: Arc<TradingEngine>;
    let mut restored = false;
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
        if reset {
            TradingEngine::clear_snapshot(mode);
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
            st.telegram.clone(),
        ));
        // Restore this mode's saved positions/cash (backend-restart recovery)
        // unless the caller explicitly asked for a reset. Only state is
        // restored — the config is the fresh one passed to `new()` above.
        restored = !reset && new_engine.restore_snapshot().await;
        new_engine.start(poll_sec, restored).await;
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
        "ok": true, "mode": mode.as_str(), "resumed": reuse, "restored_from_snapshot": restored,
        "dropped_untradeable": dropped,
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
    // 사용자가 의도적으로 정지 — 백엔드 재시작 시 자동 재개하지 않는다.
    set_autostart_enabled(false);
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

/// `POST /api/trading/telegram/report` — 현재 상태 + 모니터링 + 최근 40건
/// 거래내역을 정리해 stock_monitor 방으로 즉시 전송한다.
async fn telegram_report(State(st): State<AppState>) -> ApiResult {
    let engine = st.engine.lock().await.clone();
    match crate::telegram::send_status_report(&st.telegram, engine, &st.journal).await {
        Ok(()) => Ok(Json(json!({"ok": true}))),
        Err(e) => Err((StatusCode::BAD_GATEWAY, e)),
    }
}

/// `POST /api/trading/telegram/test` — 연결 확인용 핑 메시지 전송(설정 검증).
async fn telegram_test(State(st): State<AppState>) -> ApiResult {
    let msg = format!("✅ stock_monitor 연결 확인 — {}", crate::journal::now_iso());
    match st.telegram.send(&msg).await {
        Ok(()) => Ok(Json(json!({"ok": true}))),
        Err(e) => Err((StatusCode::BAD_GATEWAY, e)),
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
        .route("/api/trading/telegram/report", post(telegram_report))
        .route("/api/trading/telegram/test", post(telegram_test))
}
