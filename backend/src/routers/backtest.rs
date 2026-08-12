//! Backtest endpoints — watchlist-wide only (single-symbol backtest removed).
//!
//! Two surfaces, both operating over the whole watchlist (`shcodes`):
//!   - `/api/backtest/strategy-matrix` : 트레이더(전략 프리셋) × 종목 검증 + 베스트 전략 판정
//!   - `/api/backtest/batch`           : 패턴별 통계(참고용)

use crate::backtest::{
    backtest_many, bar_sigma_pct, evaluate_strategy_live, oos_layout, oos_test_bars,
    recommended_hold_bars, run_strategy_backtest, CostModel, MtfContext, MIN_OOS_SIGNALS, OOS_FOLDS,
};
use crate::candle::Candle;
use crate::pattern::ALL_PATTERNS;
use crate::risk::RiskConfig;
use crate::state::AppState;
use crate::strategy::{self, StrategyConfig};
use crate::timeframe::Timeframe;
use crate::universe::name_for;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
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

/// Build the risk config the backtest simulates.
///
/// This must be able to express the *live* settings, not just ATR multiples:
/// a legacy strategy run with a fixed 1.5% stop and `hard_stop_intrabar` off
/// behaves nothing like the same strategy on ATR stops — that gap is exactly
/// what made the realized record diverge from the backtest (stop-outs filled at
/// −3.4% against a −1.5% setting). v2 strategies overwrite these with their own
/// doctrine inside `run_strategy_backtest`, so sending them is harmless there.
fn risk_of(body: &Value) -> RiskConfig {
    let r = body.get("risk").cloned().unwrap_or(json!({}));
    let f = |k: &str, d: f64| r.get(k).and_then(Value::as_f64).unwrap_or(d);
    let i = |k: &str, d: i64| r.get(k).and_then(Value::as_i64).unwrap_or(d);
    let b = |k: &str, d: bool| r.get(k).and_then(Value::as_bool).unwrap_or(d);
    RiskConfig {
        stop_loss_atr_mult: f("stop_loss_atr_mult", 1.5),
        take_profit_atr_mult: f("take_profit_atr_mult", 3.0),
        trailing_stop_atr: f("trailing_stop_atr", 2.0),
        // Fixed-% stops (0 = use the ATR multiples above), as the engine reads them.
        stop_loss_pct: f("stop_loss_pct", 0.0),
        take_profit_pct: f("take_profit_pct", 0.0),
        // Fill model: off = the stop is only judged at the bar close, so the
        // simulated loss overshoots exactly like the live one does.
        hard_stop_intrabar: b("hard_stop_intrabar", false),
        use_take_profit: b("use_take_profit", true),
        use_trailing_stop: b("use_trailing_stop", true),
        require_confirmation: b("require_confirmation", true),
        confirm_window_bars: i("confirm_window_bars", 3),
        require_higher_tf_uptrend: b("require_higher_tf_uptrend", true),
        higher_tf_slope_tolerance: f("higher_tf_slope_tolerance", 0.0),
        eod_flatten: b("eod_flatten", true),
        ..Default::default()
    }
}

/// 이 (종목, 타임프레임) 캔들의 봉당 종가수익률 σ(%) — 캐시된 시리즈에서 바로 계산.
fn sigma_for(by_tf: &HashMap<Timeframe, Vec<Candle>>, tf: Timeframe) -> f64 {
    by_tf.get(&tf).map(|c| bar_sigma_pct(c)).unwrap_or(0.0)
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
    // 심층 조회: 연속조회(cts)로 과거를 더 끌어와 OOS 표본을 늘린다. 0/미지정이면
    // 기존 단발 조회(타임프레임 기본 예산)를 그대로 쓴다. 백테스트 전용이라
    // 실시간 엔진의 사이클 예산에는 영향을 주지 않는다.
    let history_bars = body.get("history_bars").and_then(Value::as_u64).unwrap_or(0) as usize;
    // 손절/익절 폭을 알려주면 종목별 변동성으로 권장 보유봉수를 역산해 돌려준다.
    let target_pct = body.get("target_pct").and_then(Value::as_f64).unwrap_or(0.0);
    let token = st.token().await?;
    // Both generations compete in the matrix, so the v2 presets can be compared
    // head-to-head against the legacy ones they are meant to replace. Callers
    // may narrow it with `generation: "legacy" | "v2"`.
    let presets = match body.get("generation").and_then(Value::as_str) {
        Some("legacy") => strategy::presets_of(strategy::Generation::Legacy),
        Some("v2") => strategy::presets_of(strategy::Generation::V2),
        _ => strategy::all_presets(),
    };

    // Timeframe each strategy is evaluated on.
    let tf_for = |cfg: &StrategyConfig| if auto { cfg.recommended_tf() } else { fixed_tf };

    // Per-(strategy, symbol) rows.
    let mut items: Vec<Value> = vec![];
    for code in &codes {
        let name = name_for(code);
        // Fetch candles for each distinct timeframe this run needs (cache dedups).
        let mut by_tf: HashMap<Timeframe, Vec<Candle>> = HashMap::new();
        // 상위TF 컨텍스트는 전략과 무관하게 (종목, TF) 로만 결정되므로 한 번만
        // 만들어 모든 프리셋이 공유한다 (프리셋마다 다시 만들면 10배 느려진다).
        let mut mtf_by_tf: HashMap<Timeframe, MtfContext> = HashMap::new();
        for cfg in &presets {
            let t = tf_for(cfg);
            if !by_tf.contains_key(&t) {
                let rows = if history_bars > 0 {
                    st.fetcher.fetch_history(&token, code, t, history_bars).await
                } else {
                    st.fetcher.fetch(&token, code, t).await
                };
                mtf_by_tf.insert(t, MtfContext::build(&rows, t, risk.higher_tf_slope_tolerance));
                by_tf.insert(t, rows);
            }
        }
        for cfg in &presets {
            let t = tf_for(cfg);
            let candles = by_tf.get(&t).expect("fetched above");
            if candles.len() < 30 {
                items.push(json!({
                    "strategy": cfg.name, "shcode": code, "name": name, "tf": t.as_str(),
                    "generation": cfg.generation.as_str(),
                    "ok": false, "error": "캔들 부족(30개 미만)", "tradeable": false,
                }));
                continue;
            }
            let mtf = mtf_by_tf.get(&t).expect("built above");
            let in_sample = run_strategy_backtest(candles, cfg, t, &risk, &cost, max_hold, mtf);
            let oos = evaluate_strategy_live(candles, cfg, t, &risk, &cost, max_hold, mtf);
            // 과최적화 진단: 인샘플 대비 OOS 가 얼마나 무너졌는가.
            // 1.0 = OOS 가 IS 만큼 나옴, 0 이하 = OOS 에서 수익이 사라짐.
            let is_ret = in_sample["avg_return"].as_f64().unwrap_or(0.0);
            let oos_ret = oos["oos_avg_return"].as_f64().unwrap_or(0.0);
            // IS 가 애초에 적자면 비율에 의미가 없으므로 null 로 둔다.
            let retention = if is_ret > 1e-9 { json!(oos_ret / is_ret) } else { Value::Null };
            items.push(json!({
                "strategy": cfg.name, "shcode": code, "name": name, "tf": t.as_str(), "ok": true, "error": Value::Null,
                "generation": cfg.generation.as_str(),
                "entry_threshold": cfg.entry_threshold,
                "in_sample_signals": in_sample["signals"], "in_sample_avg_return": in_sample["avg_return"],
                "in_sample_win_rate": in_sample["win_rate"], "in_sample_profit_factor": in_sample["profit_factor"],
                "oos_avg_return": oos["oos_avg_return"], "oos_consistency": oos["oos_consistency"],
                "oos_total_signals": oos["oos_total_signals"], "tradeable": oos["tradeable"],
                "oos_fold_bars": oos["oos_fold_bars"], "oos_signal_capacity": oos["oos_signal_capacity"],
                "oos_capacity_ok": oos["oos_capacity_ok"], "candles": candles.len(),
                // ── 승률 중심 선정용 신규 지표 ──
                "oos_win_rate": oos["oos_win_rate"],
                "oos_win_rate_lb": oos["oos_win_rate_lb"],
                "oos_payoff": oos["oos_payoff"],
                "oos_breakeven_win_rate": oos["oos_breakeven_win_rate"],
                "oos_win_edge": oos["oos_win_edge"],
                "oos_profit_factor": oos["oos_profit_factor"],
                "oos_worst_mdd": oos["oos_worst_mdd"],
                "is_oos_retention": retention,
                // ── 종목·TF별 실측 변동성과 그에 맞는 권장 보유봉수 ──
                "bar_sigma_pct": sigma_for(&by_tf, t),
                "recommended_hold_bars": recommended_hold_bars(target_pct, sigma_for(&by_tf, t)),
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
            "strategy": cfg.name, "entry_threshold": cfg.entry_threshold,
            "generation": cfg.generation.as_str(),
            "summary": cfg.summary,
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
        "history_bars": history_bars, "target_pct": target_pct,
        "round_trip_cost_pct": cost.round_trip_cost() * 100.0,
        "count": codes.len(),
        "strategies": presets.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        "generation": body.get("generation").and_then(Value::as_str).unwrap_or("all"),
        // Per-generation membership so the caller can group/colour the grid and
        // keep selection from mixing the two.
        "generations": {
            "v2": presets.iter().filter(|c| c.generation == strategy::Generation::V2).map(|c| c.name.clone()).collect::<Vec<_>>(),
            "legacy": presets.iter().filter(|c| c.generation == strategy::Generation::Legacy).map(|c| c.name.clone()).collect::<Vec<_>>(),
        },
        // The v2 market/relative-strength gate needs proxy candles the
        // backtester isn't given, so it is NOT simulated: live entries for a v2
        // strategy are a strict subset of the signals counted here.
        "market_filter_simulated": false,
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

/// `GET /api/backtest/capacity` — 타임프레임별로 **OOS 표본을 몇 건까지 만들 수
/// 있는지**를 미리 알려준다.
///
/// walk-forward 는 데이터를 `OOS_FOLDS+1` 등분해 뒤쪽 `OOS_FOLDS` 개를 검정 구간으로
/// 쓰고, 백테스트는 포지션을 중첩하지 않으므로 한 번 진입하면 최대 `보유봉수` 만큼
/// 봉을 소비한다. 따라서
///
/// ```text
/// 최대 OOS 신호 수 = (조회봉수 × OOS_FOLDS / (OOS_FOLDS+1)) / 보유봉수
/// ```
///
/// 이 값이 `MIN_OOS_SIGNALS` 미만이면 전략이 아무리 좋아도 `tradeable` 이 될 수 없다.
/// 화면에서 실행 전에 조합을 검증하는 용도.
async fn capacity() -> ApiResult {
    let rows: Vec<Value> = Timeframe::all()
        .iter()
        .map(|tf| {
            let candles = tf.config().qrycnt as usize;
            let (folds, fold_bars) = oos_layout(candles);
            let test_bars = oos_test_bars(candles);
            json!({
                "tf": tf.as_str(),
                "candles": candles,
                "folds": folds,
                "fold_bars": fold_bars,
                "oos_test_bars": test_bars,
                // 최소 표본을 채울 수 있는 보유봉수 상한 (이보다 크면 구조적으로 불가)
                "max_hold_for_min_signals": test_bars / MIN_OOS_SIGNALS as usize,
            })
        })
        .collect();
    Ok(Json(json!({
        "oos_folds": OOS_FOLDS,
        "min_oos_signals": MIN_OOS_SIGNALS,
        "timeframes": rows,
    })))
}

/// Backtest routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/backtest/capacity", get(capacity))
        .route("/api/backtest/strategy-matrix", post(strategy_matrix))
        .route("/api/backtest/batch", post(backtest_batch))
}
