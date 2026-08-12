//! Automated trading orchestrator (spec section 10-4).
//!
//! A background task polls the watchlist each cycle, checks exits before entries,
//! and routes orders through `Broker` after passing the `RiskManager` gate. To
//! avoid whipsaw it judges entries/exits on *closed* bars only; an optional hard
//! stop reacts to the forming bar's live price. Market-hours aware (KST).

use crate::backtest::CostModel;
use crate::broker::{Broker, TradingMode};
use crate::candle::Candle;
use crate::candle_fetcher::CandleFetcher;
use crate::ebest::EBestService;
use crate::journal::{now_iso, TradeJournal, TradeRecord};
use crate::market::{self, MarketContext};
use crate::mtf::MtfEngine;
use crate::pattern::{
    apply_strategy, compute_atr, detect_setups, detect_v2_setups, PatternDetector, PatternResult,
    SetupSeries,
};
use crate::risk::{RiskConfig, RiskManager};
use crate::session::SessionContext;
use crate::strategy::{Source, StrategyConfig};
use crate::timeframe::Timeframe;
use crate::universe::name_for;
use chrono::{Datelike, NaiveDate, Timelike, Utc, Weekday};
use chrono_tz::Asia::Seoul;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Order-execution settings (per-order qty, type, sell-all, portfolio buy cap).
#[derive(Clone)]
pub struct OrderConfig {
    pub order_type: String,
    pub fixed_qty: Option<i64>,
    pub sell_all: bool,
    /// Cap on **total entered amount** across all open positions (Σ entry×qty)
    /// — not a per-order limit. New entries and averaging-down adds are sized
    /// down so the running total never exceeds this (0 = uncapped).
    pub max_buy_amount: f64,
}

impl OrderConfig {
    fn to_json(&self) -> Value {
        json!({
            "order_type": self.order_type,
            "fixed_qty": self.fixed_qty.unwrap_or(0),
            "sell_all": self.sell_all,
            "max_buy_amount": self.max_buy_amount,
        })
    }
}

/// Outcome of a single entry evaluation — drives the per-symbol monitoring view.
struct Eval {
    phase: &'static str, // 짧은 상태 라벨 (보유/신호없음/확인대기/...)
    detail: String,      // 사람이 읽는 사유
    score: Option<f64>,  // 종합점수 (패턴 감지 시)
    pattern: Option<String>,
}

impl Eval {
    fn new(phase: &'static str, detail: impl Into<String>) -> Self {
        Self { phase, detail: detail.into(), score: None, pattern: None }
    }
    fn with_signal(phase: &'static str, detail: impl Into<String>, score: f64, pattern: &str) -> Self {
        Self { phase, detail: detail.into(), score: Some(score), pattern: Some(pattern.to_string()) }
    }
}

/// Convert a candle's date/time fields to a UTC Unix timestamp (chart markers).
fn candle_unix_ts(c: &Candle) -> i64 {
    let date = c.date.replace('-', "");
    let time = c.time.replace(':', "");
    if date.len() == 8 {
        if let Ok(d) = NaiveDate::parse_from_str(&date, "%Y%m%d") {
            let hh = if time.len() >= 4 { time[..2].parse().unwrap_or(9) } else { 9 };
            let mm = if time.len() >= 4 { time[2..4].parse().unwrap_or(0) } else { 0 };
            if let Some(dt) = d.and_hms_opt(hh, mm, 0) {
                return dt.and_utc().timestamp();
            }
        }
    }
    kst_wallclock_unix()
}

/// Current KST wall-clock encoded as if it were UTC — the same convention
/// `candle_unix_ts` applies to candle date/time fields, so chart markers align
/// with the candle time scale instead of being shifted by 9 hours.
fn kst_wallclock_unix() -> i64 {
    Utc::now().with_timezone(&Seoul).naive_local().and_utc().timestamp()
}

fn hhmm_label() -> String {
    Utc::now().with_timezone(&Seoul).format("%H:%M").to_string()
}

/// Shared dependencies (network clients + immutable helpers) for the engine.
struct Deps {
    ebest: Option<Arc<EBestService>>,
    fetcher: Arc<CandleFetcher>,
    detector: PatternDetector,
    broker: Broker,
    journal: Arc<TradeJournal>,
    cost: CostModel,
    /// 거래 이벤트(진입/청산/물타기/일손실한도) 자동 알림용 — 미설정이면 무시.
    notifier: Arc<crate::telegram::TelegramNotifier>,
}

/// 거래 알림을 stock_monitor 방으로 비동기 전송한다 (루프를 블로킹하지 않음).
fn notify(deps: &Deps, msg: String) {
    if !deps.notifier.configured() {
        return;
    }
    let n = deps.notifier.clone();
    tokio::spawn(async move {
        if let Err(e) = n.send(&msg).await {
            tracing::warn!("[telegram] 거래 알림 전송 실패: {e}");
        }
    });
}

/// 알림용 모드 라벨.
fn mode_label(mode: TradingMode) -> &'static str {
    if mode == TradingMode::Live { "실전" } else { "모의" }
}

/// Mutable engine state guarded by the engine mutex.
struct State {
    strategy: StrategyConfig,
    /// Per-symbol strategy overrides (code → strategy). Falls back to `strategy`.
    /// Each symbol can be traded with the strategy that backtested best for it,
    /// on that strategy's recommended timeframe.
    symbol_strategies: HashMap<String, StrategyConfig>,
    risk: RiskManager,
    watchlist: Vec<String>,
    tf: Timeframe,
    ignore_market_hours: bool,
    poll_sec: u64,
    seed_cash: f64,
    order: OrderConfig,
    cash: f64,
    running: bool,
    cycles: u64,
    last_error: Option<String>,
    opened_meta: HashMap<String, (String, String)>, // code -> (opened_at, pattern)
    last_bar_ts: HashMap<String, i64>,
    cooldown: HashMap<String, i64>,
    /// Stop-out re-buy guard: code → (exit fill price, bars until the guard
    /// expires; 0 = never expires). Decremented once per closed bar.
    last_exit_price: HashMap<String, (f64, i64)>,
    bars_held: HashMap<String, i64>,
    current_prices: HashMap<String, f64>,
    /// 전일(이월) 스냅샷에서 복원된 포지션 — 엔진이 꺼진 동안 EOD 청산을 놓친
    /// 것이므로 `eod_flatten` 이 켜져 있으면 첫 사이클에 즉시 청산한다.
    /// (같은 날 재시작 복원은 해당 없음 — 정상 관리 계속.)
    flatten_stale: HashSet<String>,
    /// Codes with a close order in flight — blocks a second concurrent close
    /// (manual vs automatic) from double-selling the same position.
    closing: HashSet<String>,
    /// 일손실 한도 도달 알림을 하루 1회로 제한 (일일 리셋 시 해제).
    loss_limit_notified: bool,
    /// Live only: account holdings (t0424) the engine does not track — surfaced
    /// in status so orphaned/manual holdings are visible instead of silent.
    unmanaged_holdings: Vec<String>,
    trade_events: Vec<Value>,
    monitor: HashMap<String, Value>, // code -> latest per-cycle monitoring snapshot
    /// Latest broad-market read, populated only while a v2 strategy with a
    /// market filter is running (surfaced in status so the operator can see why
    /// entries are being held back).
    market: Option<MarketContext>,
}

/// On-disk engine snapshot (per mode) so a backend restart doesn't orphan open
/// positions — without this, live holdings would lose all stop/target
/// management the moment the process dies.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    /// KST date (YYYY-MM-DD) the snapshot was written; daily P&L is only
    /// restored for a same-day restart.
    day: String,
    cash: f64,
    seed_cash: f64,
    daily_pnl: f64,
    day_start_equity: f64,
    positions: HashMap<String, crate::risk::Position>,
    opened_meta: HashMap<String, (String, String)>,
    cooldown: HashMap<String, i64>,
    last_exit_price: HashMap<String, (f64, i64)>,
    bars_held: HashMap<String, i64>,
    /// 종목별로 마지막으로 *처리한* 닫힌 봉의 타임스탬프. 이걸 저장하지 않으면
    /// 재시작 직후 모든 종목이 `missed=1`(새 봉)로 평가돼 직전 세션 마감봉으로
    /// 진입·청산이 다시 한 번 발생한다. 구버전 스냅샷 호환을 위해 `default`.
    #[serde(default)]
    last_bar_ts: HashMap<String, i64>,
}

fn snapshot_path(mode: TradingMode) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("data/engine_state_{}.json", mode.as_str()))
}

fn kst_today() -> String {
    Utc::now().with_timezone(&Seoul).format("%Y-%m-%d").to_string()
}

/// Upper bound on how many missed bars one cycle will compensate for — bounds
/// both the pattern-scan window and the cooldown/guard catch-up per cycle.
const MISSED_BARS_CAP: i64 = 10;

/// A timeframe's bar length in seconds (daily counts as one calendar day).
fn tf_secs(tf: Timeframe) -> i64 {
    match tf {
        Timeframe::D1 => 86_400,
        _ => tf.config().ncnt as i64 * 60,
    }
}

/// 캐시 TTL(최대 180초) + 대형 워치리스트 사이클 지연을 흡수하는 여유분.
const STALE_BAR_SLACK_SECS: i64 = 300;

/// 마지막 닫힌 봉이 이보다 오래되면 "정지된 데이터"로 보고 매매 판정을 건너뛴다.
/// 장 마감 후·휴장일·데이터 피드 장애 상황에서 직전 세션 종가로 진입/청산이
/// 체결되는 것을 막는 최종 방어선 — `ignore_market_hours` 설정과 무관하게 적용된다.
fn stale_after_secs(tf: Timeframe) -> i64 {
    match tf {
        // 일봉은 주말·연휴를 건너뛰므로 넉넉히 잡는다.
        Timeframe::D1 => 5 * 86_400,
        _ => tf_secs(tf) * 3 + STALE_BAR_SLACK_SECS,
    }
}

impl State {
    /// Equity = cash + position marks (every position is a long, valued at market).
    fn equity(&self) -> f64 {
        let held: f64 = self
            .risk
            .open_positions()
            .iter()
            .map(|(code, p)| {
                let cur = self.current_prices.get(code).copied().unwrap_or(p.entry);
                p.qty as f64 * cur
            })
            .sum();
        self.cash + held
    }

    fn append_event(&mut self, event: Value) {
        self.trade_events.push(event);
        if self.trade_events.len() > 500 {
            self.trade_events.drain(..self.trade_events.len() - 500);
        }
    }

    /// Strategy used for `code`: the per-symbol override, else the global strategy.
    fn strategy_for(&self, code: &str) -> StrategyConfig {
        self.symbol_strategies.get(code).cloned().unwrap_or_else(|| self.strategy.clone())
    }

    /// Risk rules `code` is actually traded under: the operator's base config
    /// with its strategy's overrides applied. Legacy strategies override
    /// nothing, so they get the base config unchanged.
    fn risk_for(&self, code: &str) -> RiskConfig {
        self.strategy_for(code).effective_risk(&self.risk.cfg)
    }

    /// Timeframe `code` trades on: the assigned strategy's recommended TF, else the global TF.
    fn tf_for(&self, code: &str) -> Timeframe {
        self.symbol_strategies.get(code).map(|c| c.recommended_tf()).unwrap_or(self.tf)
    }

    /// Drop per-symbol state for codes no longer watched and not held.
    /// Called after a watchlist change so removed symbols fully fall out of the
    /// engine, while open positions are always retained (kept whether watched or not).
    fn prune_stale(&mut self) {
        let keep: std::collections::HashSet<String> = self
            .watchlist
            .iter()
            .cloned()
            .chain(self.risk.open_positions().keys().cloned())
            .collect();
        self.opened_meta.retain(|c, _| keep.contains(c));
        self.last_bar_ts.retain(|c, _| keep.contains(c));
        self.cooldown.retain(|c, _| keep.contains(c));
        self.last_exit_price.retain(|c, _| keep.contains(c));
        self.bars_held.retain(|c, _| keep.contains(c));
        self.current_prices.retain(|c, _| keep.contains(c));
        self.monitor.retain(|c, _| keep.contains(c));
        self.symbol_strategies.retain(|c, _| keep.contains(c));
    }

    /// Write the current positions/cash/state to disk (crash/restart recovery).
    /// Called after every trade mutation and at each cycle end — cheap (small JSON).
    fn persist(&self, mode: TradingMode) {
        let snap = Snapshot {
            day: kst_today(),
            cash: self.cash,
            seed_cash: self.seed_cash,
            daily_pnl: self.risk.daily_pnl(),
            day_start_equity: self.risk.day_start_equity(),
            positions: self.risk.open_positions().clone(),
            opened_meta: self.opened_meta.clone(),
            cooldown: self.cooldown.clone(),
            last_exit_price: self.last_exit_price.clone(),
            bars_held: self.bars_held.clone(),
            last_bar_ts: self.last_bar_ts.clone(),
        };
        let path = snapshot_path(mode);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string(&snap) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    tracing::warn!("engine snapshot write failed: {e}");
                }
            }
            Err(e) => tracing::warn!("engine snapshot serialize failed: {e}"),
        }
    }

    /// Store this cycle's monitoring snapshot for one symbol (overwrites the previous).
    fn record_monitor(&mut self, code: &str, bar_price: f64, live_price: f64, atr: f64, e: Eval) {
        self.monitor.insert(code.to_string(), json!({
            "code": code,
            "name": name_for(code),
            "at": hhmm_label(),
            "bar_price": bar_price.round(),
            "live_price": live_price.round(),
            "atr": (atr * 100.0).round() / 100.0,
            "phase": e.phase,
            "detail": e.detail,
            "score": e.score.map(|v| (v * 1000.0).round() / 1000.0),
            "pattern": e.pattern,
        }));
    }
}

/// The trading engine: dependencies + shared state + the loop task handle.
pub struct TradingEngine {
    deps: Arc<Deps>,
    state: Arc<Mutex<State>>,
    mode: TradingMode,
    task: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl TradingEngine {
    /// Build a fresh engine (called for a first start or a reset).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ebest: Option<Arc<EBestService>>,
        fetcher: Arc<CandleFetcher>,
        detector: PatternDetector,
        broker: Broker,
        strategy: StrategyConfig,
        symbol_strategies: HashMap<String, StrategyConfig>,
        risk: RiskManager,
        watchlist: Vec<String>,
        tf: Timeframe,
        ignore_market_hours: bool,
        journal: Arc<TradeJournal>,
        seed_cash: f64,
        order: OrderConfig,
        notifier: Arc<crate::telegram::TelegramNotifier>,
    ) -> Self {
        let mode = broker.mode;
        let deps = Arc::new(Deps { ebest, fetcher, detector, broker, journal, cost: CostModel::default(), notifier });
        let state = Arc::new(Mutex::new(State {
            strategy,
            symbol_strategies,
            risk,
            watchlist,
            tf,
            ignore_market_hours,
            poll_sec: 60,
            seed_cash,
            order,
            cash: seed_cash,
            running: false,
            cycles: 0,
            last_error: None,
            opened_meta: HashMap::new(),
            last_bar_ts: HashMap::new(),
            cooldown: HashMap::new(),
            last_exit_price: HashMap::new(),
            bars_held: HashMap::new(),
            current_prices: HashMap::new(),
            flatten_stale: HashSet::new(),
            closing: HashSet::new(),
            loss_limit_notified: false,
            unmanaged_holdings: vec![],
            trade_events: vec![],
            monitor: HashMap::new(),
            market: None,
        }));
        Self { deps, state, mode, task: std::sync::Mutex::new(None) }
    }

    pub fn mode(&self) -> TradingMode {
        self.mode
    }

    /// Restore a saved snapshot for this mode, if one exists. Returns true when
    /// state was restored — the caller then starts with `resume=true` so the
    /// restored cash isn't re-seeded. Daily P&L only survives a same-day restart.
    pub async fn restore_snapshot(&self) -> bool {
        let Ok(raw) = std::fs::read_to_string(snapshot_path(self.mode)) else {
            return false;
        };
        let Ok(snap) = serde_json::from_str::<Snapshot>(&raw) else {
            tracing::warn!("engine snapshot unreadable — starting fresh");
            return false;
        };
        let mut s = self.state.lock().await;
        s.cash = snap.cash;
        s.seed_cash = snap.seed_cash;
        s.opened_meta = snap.opened_meta;
        s.cooldown = snap.cooldown;
        s.last_exit_price = snap.last_exit_price;
        s.bars_held = snap.bars_held;
        // 이미 처리한 봉을 다시 거래하지 않도록 복원 (재시작 중복 진입/청산 방지).
        s.last_bar_ts = snap.last_bar_ts;
        let n = snap.positions.len();
        s.risk.restore_positions(snap.positions);
        if snap.day == kst_today() {
            s.risk.set_daily_pnl(snap.daily_pnl);
            s.risk.set_day_start_equity(snap.day_start_equity);
        } else {
            let eq = s.equity();
            s.risk.reset_daily(eq);
            // 봉수 기반 쿨다운/재매수 가격가드는 지난 세션의 것 — 새 거래일에는
            // 의미가 없고(주말·휴일을 건너뛴 봉 계산이 불확정), 새로 평가한다.
            s.cooldown.clear();
            s.last_exit_price.clear();
            // 전일 이월 포지션: 엔진이 꺼져 있어 EOD 청산(15:10)을 놓친 것.
            // 단타 설계상 밤새 갭 리스크에 노출된 상태이므로 표시해 두고,
            // `eod_flatten` 이 켜져 있으면 첫 사이클에 즉시 청산한다.
            s.flatten_stale = s.risk.open_positions().keys().cloned().collect();
            if !s.flatten_stale.is_empty() {
                tracing::warn!(
                    "[engine] 전일({}) 이월 포지션 {}건 복원 — eod_flatten 설정 시 장 시작 후 즉시 청산: {:?}",
                    snap.day, s.flatten_stale.len(), s.flatten_stale
                );
            }
        }
        tracing::info!("[engine] snapshot restored ({} mode, {n} positions, cash={:.0})", self.mode.as_str(), s.cash);
        true
    }

    /// Delete the on-disk snapshot for a mode (used by an explicit reset).
    pub fn clear_snapshot(mode: TradingMode) {
        let _ = std::fs::remove_file(snapshot_path(mode));
    }

    pub async fn running(&self) -> bool {
        self.state.lock().await.running
    }

    /// Update config without touching positions/cash/events (reconfigure on resume).
    #[allow(clippy::too_many_arguments)]
    pub async fn reconfigure(
        &self,
        strategy: StrategyConfig,
        symbol_strategies: HashMap<String, StrategyConfig>,
        watchlist: Vec<String>,
        tf: Timeframe,
        ignore_market_hours: bool,
        order: OrderConfig,
        risk_cfg: RiskConfig,
        seed_cash: f64,
    ) {
        let mut s = self.state.lock().await;
        s.strategy = strategy;
        s.symbol_strategies = symbol_strategies;
        s.watchlist = watchlist;
        s.tf = tf;
        s.ignore_market_hours = ignore_market_hours;
        s.order = order;
        s.risk.cfg = risk_cfg;
        s.seed_cash = seed_cash;
        s.prune_stale();
    }

    /// Replace just the live strategy weights (PUT /trading/strategy).
    pub async fn set_strategy(&self, strategy: StrategyConfig) -> String {
        let mut s = self.state.lock().await;
        let name = strategy.name.clone();
        s.strategy = strategy;
        name
    }

    /// Spawn the polling loop (idempotent while already running).
    pub async fn start(&self, poll_sec: u64, resume: bool) {
        if self.state.lock().await.running {
            return;
        }
        // Wait for the previous loop task (if any) to fully exit first — a quick
        // stop→start could otherwise flip `running` back to true while the old
        // loop is still winding down, leaving two loops trading the same state.
        let old = self.task.lock().unwrap().take();
        if let Some(h) = old {
            let _ = h.await;
        }
        {
            let mut s = self.state.lock().await;
            if s.running {
                return; // another start won the race while we awaited
            }
            s.running = true;
            s.poll_sec = poll_sec;
        }
        let deps = self.deps.clone();
        let state = self.state.clone();
        let handle = tokio::spawn(async move { run_loop(deps, state, poll_sec, resume).await });
        *self.task.lock().unwrap() = Some(handle);
    }

    /// Stop the loop. The task is *not* aborted: killing it between sending an
    /// order and updating internal state would desync positions/cash from the
    /// real account. The loop observes `running=false` (checked every second
    /// during its sleep) and exits on its own; the handle stays in `task` so
    /// the next `start()` can await its completion.
    pub async fn stop(&self) {
        self.state.lock().await.running = false;
    }

    /// Clear this session's in-memory trade events; returns the count removed.
    pub async fn clear_events(&self) -> usize {
        let mut s = self.state.lock().await;
        let n = s.trade_events.len();
        s.trade_events.clear();
        n
    }

    /// Manually close a held position in full (same path as an automatic exit).
    pub async fn manual_close(&self, code: &str) -> Value {
        let token = resolve_token(&self.deps).await;
        // Snapshot the entry/qty before any reduce, plus a fallback price.
        let (mut price, mut candle_ts);
        {
            let s = self.state.lock().await;
            if s.closing.contains(code) {
                return json!({"ok": false, "reason": "close_in_progress"});
            }
            if s.risk.position(code).is_none() {
                return json!({"ok": false, "reason": "no_position"});
            }
            price = s.current_prices.get(code).copied().unwrap_or_else(|| s.risk.position(code).unwrap().entry);
            candle_ts = kst_wallclock_unix();
        }
        // Close on the same timeframe the symbol actually trades on.
        let tf = self.state.lock().await.tf_for(code);
        let candles = self.deps.fetcher.fetch(&token, code, tf).await;
        if let Some(last) = candles.last() {
            price = last.close;
            candle_ts = candle_unix_ts(last);
        }
        let mut s = self.state.lock().await;
        // Re-check + arm the in-flight guard under one lock: the automatic exit
        // path also sells outside the lock, and both closing the same position
        // would double-sell it (a real order the live account can't cover).
        if s.closing.contains(code) {
            return json!({"ok": false, "reason": "close_in_progress"});
        }
        let Some(pos) = s.risk.position(code).cloned() else {
            return json!({"ok": false, "reason": "no_position"});
        };
        let (qty, entry) = (pos.qty, pos.entry);
        let order_type = s.order.order_type.clone();
        s.closing.insert(code.to_string());
        drop(s);
        let fill = self.deps.broker.sell(&token, code, qty, price, &order_type).await;
        if !fill.ok || fill.qty <= 0 {
            self.state.lock().await.closing.remove(code);
            return json!({"ok": false, "reason": "order_failed"});
        }
        // 지정가 부분 체결 대비: 실제 체결수량 기준으로 정산한다.
        let sold = fill.qty.min(qty);
        let mut s = self.state.lock().await;
        s.closing.remove(code);
        let pnl = (fill.fill_price - entry) * sold as f64;
        s.cash += sold as f64 * fill.fill_price;
        let pnl_pct = if entry != 0.0 { (fill.fill_price - entry) / entry * 100.0 } else { 0.0 };
        let fully = sold >= qty;
        record_close(&self.deps, &mut s, code, &pos, fill.fill_price, "manual_close", sold, fully);
        s.risk.reduce(code, sold, pnl);
        s.append_event(json!({
            "code": code, "name": name_for(code),
            "type": "sell", "action": "close",
            "price": fill.fill_price,
            "qty": sold, "pnl": pnl.round(), "pnl_pct": (pnl_pct * 100.0).round() / 100.0,
            "reason": "manual_close", "ts": candle_ts, "time_label": hhmm_label(),
        }));
        s.persist(self.deps.broker.mode);
        tracing::info!("MANUAL CLOSE {code} x{sold} @{:.0} pnl={:.0}", fill.fill_price, pnl);
        notify(&self.deps, format!(
            "🔴 [{}] 수동 청산 — {}({code}) {sold}주 @{:.0}\n손익 {:+.0}원 ({pnl_pct:+.2}%)",
            mode_label(self.deps.broker.mode), name_for(code), fill.fill_price, pnl
        ));
        json!({"ok": true, "fill_price": fill.fill_price, "qty": sold,
               "pnl": pnl.round(), "pnl_pct": (pnl_pct * 100.0).round() / 100.0})
    }

    /// Full engine status snapshot (positions, equity, events) for the API.
    pub async fn status(&self) -> Value {
        let s = self.state.lock().await;
        let positions: Vec<Value> = s
            .risk
            .open_positions()
            .iter()
            .map(|(code, p)| {
                let cur = s.current_prices.get(code).copied().unwrap_or(p.entry);
                let upnl = (cur - p.entry) * p.qty as f64;
                let upct = if p.entry != 0.0 { (cur - p.entry) / p.entry * 100.0 } else { 0.0 };
                let meta = s.opened_meta.get(code);
                let strat = s.strategy_for(code);
                let eff = strat.effective_risk(&s.risk.cfg);
                // Risk taken and reward left, in R multiples — lets the UI show
                // "where in the trade are we" instead of just a won/lost colour.
                let risk_amt = (p.entry - p.stop).max(0.0);
                let r_multiple = if risk_amt > 0.0 { (cur - p.entry) / risk_amt } else { 0.0 };
                let span = (p.target - p.stop).max(1e-9);
                json!({
                    "code": code, "name": name_for(code),
                    "entry": p.entry, "qty": p.qty, "stop": p.stop, "target": p.target,
                    "peak": p.peak, "base_qty": p.base_qty, "fib_level": p.fib_level,
                    "buy_price": p.entry.round(), "current_price": cur.round(),
                    "unrealized_pnl": upnl.round(), "unrealized_pct": (upct * 100.0).round() / 100.0,
                    "pattern": meta.map(|m| m.1.clone()).unwrap_or_default(),
                    "opened_at": meta.map(|m| m.0.clone()).unwrap_or_default(),
                    "strategy": strat.name, "tf": s.tf_for(code).as_str(),
                    "generation": strat.generation.as_str(),
                    // Position of the live price on the stop→target axis (0..1).
                    "progress": (((cur - p.stop) / span).clamp(0.0, 1.0) * 1000.0).round() / 1000.0,
                    "r_multiple": (r_multiple * 100.0).round() / 100.0,
                    "stop_pct": if p.entry > 0.0 { ((p.stop - p.entry) / p.entry * 10000.0).round() / 100.0 } else { 0.0 },
                    "target_pct": if p.entry > 0.0 { ((p.target - p.entry) / p.entry * 10000.0).round() / 100.0 } else { 0.0 },
                    "use_take_profit": eff.use_take_profit,
                    "use_trailing_stop": eff.use_trailing_stop,
                    "hard_stop_intrabar": eff.hard_stop_intrabar,
                    "bars_held": s.bars_held.get(code).copied().unwrap_or(0),
                })
            })
            .collect();
        let events = &s.trade_events;
        let recent = &events[events.len().saturating_sub(100)..];
        let weights: serde_json::Map<String, Value> = Source::all()
            .iter()
            .map(|src| (src.as_str().to_string(), json!(s.strategy.weights.get(src).copied().unwrap_or(0.0))))
            .collect();
        let mut monitor: Vec<Value> = s.monitor.values().cloned().collect();
        monitor.sort_by(|a, b| a["code"].as_str().unwrap_or("").cmp(b["code"].as_str().unwrap_or("")));
        // Funnel diagnostics: how many symbols sit in each phase right now —
        // shows at a glance which gate is filtering the watchlist.
        let mut monitor_summary: HashMap<String, usize> = HashMap::new();
        for m in s.monitor.values() {
            *monitor_summary.entry(m["phase"].as_str().unwrap_or("-").to_string()).or_insert(0) += 1;
        }
        // Per-symbol strategy assignments (code → {strategy, tf}) for the UI.
        let symbol_strategies: serde_json::Map<String, Value> = s
            .symbol_strategies
            .iter()
            .map(|(code, cfg)| {
                (
                    code.clone(),
                    json!({
                        "strategy": cfg.name,
                        "tf": cfg.recommended_tf().as_str(),
                        "generation": cfg.generation.as_str(),
                    }),
                )
            })
            .collect();
        // The risk config the *global* strategy actually trades with, after its
        // own overrides — this is what the operator needs to see, not the raw
        // form values a v2 preset has already replaced.
        let effective_risk = s.strategy.effective_risk(&s.risk.cfg);
        json!({
            "running": s.running,
            "mode": self.mode.as_str(),
            "strategy": s.strategy.name,
            "generation": s.strategy.generation.as_str(),
            "strategy_summary": s.strategy.summary,
            "market_filter": s.strategy.to_json()["market_filter"].clone(),
            "market": s.market.as_ref().map(|m| m.to_json()),
            "timeframe": s.tf.as_str(),
            "symbol_strategies": symbol_strategies,
            "watchlist": s.watchlist,
            "poll_sec": s.poll_sec,
            "ignore_market_hours": s.ignore_market_hours,
            "seed_cash": s.seed_cash.round(),
            "entry_threshold": s.strategy.entry_threshold,
            "weights": weights,
            "cycles": s.cycles,
            "cash": s.cash.round(),
            "equity": s.equity().round(),
            "daily_pnl": s.risk.daily_pnl().round(),
            "positions": positions,
            "trade_events": recent,
            "monitor": monitor,
            "monitor_summary": monitor_summary,
            "order": s.order.to_json(),
            "risk": effective_risk.to_json(),
            "risk_form": s.risk.cfg.to_json(),
            "risk_overrides": s.strategy.to_json()["risk_overrides"].clone(),
            "unmanaged_holdings": s.unmanaged_holdings,
            "last_error": s.last_error,
        })
    }
}

/// True current price via t1101 — bypasses the candle cache (which can be up
/// to its TTL stale) for the intrabar hard stop.
async fn fresh_price(deps: &Deps, token: &str, code: &str) -> Option<f64> {
    let ebest = deps.ebest.as_ref()?;
    let q = ebest.stock_price(token, code).await;
    q.get("price").and_then(crate::ebest::parse_float).filter(|p| *p > 0.0)
}

/// Resolve an eBest auth token (empty when no client is configured).
async fn resolve_token(deps: &Deps) -> String {
    match &deps.ebest {
        Some(e) => e.auth_token(false).await.unwrap_or_default(),
        None => String::new(),
    }
}

/// KRX holidays (YYYYMMDD, comma-separated) from `TRADING_HOLIDAYS`, parsed once.
fn holidays() -> &'static HashSet<String> {
    static H: OnceLock<HashSet<String>> = OnceLock::new();
    H.get_or_init(|| {
        std::env::var("TRADING_HOLIDAYS")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    })
}

/// True when the KST market is open (Mon–Fri 09:00–15:20, not a listed
/// holiday), or hours are ignored. Without the weekday check the engine would
/// treat Friday's last bar as freshly closed on Saturday and enter on stale data.
fn market_open(s: &State) -> bool {
    if s.ignore_market_hours {
        return true;
    }
    let now = Utc::now().with_timezone(&Seoul);
    if matches!(now.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    if holidays().contains(&now.format("%Y%m%d").to_string()) {
        return false;
    }
    let mins = now.hour() * 60 + now.minute();
    (9 * 60..=15 * 60 + 20).contains(&mins)
}

/// The background polling loop.
async fn run_loop(deps: Arc<Deps>, state: Arc<Mutex<State>>, poll_sec: u64, resume: bool) {
    if !resume {
        let token = resolve_token(&deps).await;
        let mut s = state.lock().await;
        s.cash = seed_equity(&deps, &token, s.seed_cash).await;
        let eq = s.equity();
        s.risk.reset_daily(eq);
        s.loss_limit_notified = false;
    }
    let mut last_day = Utc::now().with_timezone(&Seoul).date_naive();
    loop {
        {
            let s = state.lock().await;
            if !s.running {
                break;
            }
        }
        // KST 날짜가 바뀌면 일일 손익/손실한도를 리셋한다 (엔진을 며칠 켜두는 경우).
        // 봉수 기반 쿨다운/재매수 가격가드도 함께 초기화 — 세션 경계를 넘긴 가드는
        // 주말·휴일 봉 계산이 불확정하고(missed-bars 캡), 단타에선 새 세션에서
        // 새로 평가하는 것이 맞다.
        let today = Utc::now().with_timezone(&Seoul).date_naive();
        if today != last_day {
            last_day = today;
            let mut s = state.lock().await;
            let eq = s.equity();
            s.risk.reset_daily(eq);
            s.cooldown.clear();
            s.last_exit_price.clear();
            s.loss_limit_notified = false;
        }
        let is_open = {
            let g = state.lock().await;
            market_open(&g)
        };
        if is_open {
            // The token is cached by EBestService, so re-resolving each cycle is
            // free while it is valid and transparently refreshes it once expired
            // (avoids every TR call paying a fail→refresh→retry round trip).
            let token = resolve_token(&deps).await;
            match scan_and_trade(&deps, &state, &token).await {
                Ok(()) => {
                    let mut s = state.lock().await;
                    s.cycles += 1;
                    s.last_error = None;
                }
                Err(e) => {
                    let mut s = state.lock().await;
                    s.last_error = Some(e.clone());
                    tracing::error!("trade loop error: {e}");
                }
            }
            // Live: reconcile internal positions with the real account so fill
            // slippage / external sells / orphaned holdings can't silently
            // desync the engine from reality.
            if deps.broker.mode == TradingMode::Live {
                reconcile_live(&deps, &state, &token).await;
            }
            // Persist after each cycle so a crash/restart can restore positions.
            {
                let s = state.lock().await;
                s.persist(deps.broker.mode);
            }
        }
        // Sleep in 1s slices so a stop request takes effect promptly instead of
        // lingering for up to a full poll interval.
        let mut slept = 0u64;
        while slept < poll_sec.max(1) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            slept += 1;
            let stopped = {
                let s = state.lock().await;
                if !s.running {
                    s.persist(deps.broker.mode);
                }
                !s.running
            };
            if stopped {
                return;
            }
        }
    }
    let s = state.lock().await;
    s.persist(deps.broker.mode);
}

/// Seed cash: paper uses the virtual seed; live reads the account deposit.
async fn seed_equity(deps: &Deps, token: &str, seed_cash: f64) -> f64 {
    if deps.broker.mode == TradingMode::Live {
        if let Some(ebest) = &deps.ebest {
            let bal = ebest.get_account_balance(token).await;
            let val = bal.get("t0424OutBlock").and_then(|b| b.get("sunamt")).and_then(crate::ebest::parse_float).unwrap_or(0.0);
            if val > 0.0 {
                return val;
            }
        }
    }
    seed_cash
}

/// Live-mode account reconciliation (t0424): adopt the account's actual
/// qty/average price (fill slippage correction), forget positions that no
/// longer exist in the account, and surface holdings the engine doesn't track.
/// Internal cash is intentionally *not* overwritten — t0424's 예수금 moves on
/// D+2 settlement and would whipsaw the sizing math.
async fn reconcile_live(deps: &Deps, state: &Arc<Mutex<State>>, token: &str) {
    let Some(ebest) = &deps.ebest else { return };
    let bal = ebest.get_account_balance(token).await;
    let Some(rows) = bal.get("t0424OutBlock1").and_then(Value::as_array) else {
        return; // account query failed — keep internal state, try next cycle
    };
    let mut acct: HashMap<String, (f64, i64)> = HashMap::new();
    for r in rows {
        let code = r
            .get("expcode")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .trim_start_matches('A')
            .to_string();
        let qty = r.get("janqty").and_then(crate::ebest::parse_float).unwrap_or(0.0) as i64;
        let avg = r.get("pamt").and_then(crate::ebest::parse_float).unwrap_or(0.0);
        if !code.is_empty() && qty > 0 {
            acct.insert(code, (avg, qty));
        }
    }
    let mut s = state.lock().await;
    let tracked: Vec<(String, crate::risk::Position)> = s
        .risk
        .open_positions()
        .iter()
        .map(|(c, p)| (c.clone(), p.clone()))
        .collect();
    for (code, pos) in &tracked {
        if s.closing.contains(code) {
            continue; // in-flight closes settle next cycle
        }
        match acct.get(code) {
            Some((avg, qty)) => {
                let price_drift = *avg > 0.0 && (pos.entry - avg).abs() / avg > 0.0005;
                if pos.qty != *qty || price_drift {
                    tracing::warn!(
                        "[reconcile] {code}: internal {}주@{:.0} → account {}주@{:.0} 로 보정",
                        pos.qty, pos.entry, qty, avg
                    );
                    let entry = if *avg > 0.0 { *avg } else { pos.entry };
                    s.risk.sync_position(code, entry, *qty);
                }
            }
            None => {
                tracing::warn!("[reconcile] {code}: 계좌에 없음 — 내부 포지션 제거 (외부 매도/미체결 추정)");
                s.risk.forget(code);
                s.opened_meta.remove(code);
                s.bars_held.remove(code);
            }
        }
    }
    let tracked_codes: HashSet<&String> = tracked.iter().map(|(c, _)| c).collect();
    let mut unmanaged: Vec<String> = acct.keys().filter(|c| !tracked_codes.contains(c)).cloned().collect();
    unmanaged.sort();
    s.unmanaged_holdings = unmanaged;
}

/// End-of-day handling phase for the current KST time (day-trading flatten).
#[derive(PartialEq, Clone, Copy)]
enum EodPhase {
    Normal,
    NoEntry, // close is near — hold/manage but no new entries
    Flatten, // force-close everything before the session ends
}

/// EOD phase: entries stop at 15:05, everything is flattened between 15:10 and
/// 16:00 (the engine's session ends 15:20).
///
/// `ignore_market_hours` 는 더 이상 이 게이트를 끄지 않는다 — 장시간 무시는
/// "장외에도 루프를 돌린다"는 뜻이지 "마감 리스크 관리를 끈다"는 뜻이 아니며,
/// 실제로 그 조합이 밤샘 보유 → 갭 손실의 주원인이었다. 저녁 내내 청산 상태로
/// 눌러앉지 않도록 강제청산 구간에는 상한(16:00)을 둔다.
fn eod_phase(s: &State) -> EodPhase {
    if !s.risk.cfg.eod_flatten {
        return EodPhase::Normal;
    }
    let now = Utc::now().with_timezone(&Seoul);
    let mins = now.hour() * 60 + now.minute();
    if (15 * 60 + 10..16 * 60).contains(&mins) {
        EodPhase::Flatten
    } else if mins >= 15 * 60 + 5 {
        EodPhase::NoEntry // 마감 임박 이후 ~ 자정까지 신규 진입 없음
    } else {
        EodPhase::Normal
    }
}

/// One full cycle over the watchlist + held codes.
async fn scan_and_trade(deps: &Deps, state: &Arc<Mutex<State>>, token: &str) -> Result<(), String> {
    // Snapshot codes under the lock (timeframe is resolved per symbol below).
    // Held positions come *first* — their stop/exit checks must not wait behind
    // a long watchlist scan (each fetch pays the ~1.1s/call eBest rate limit).
    let codes = {
        let s = state.lock().await;
        let mut codes: Vec<String> = s.risk.open_positions().keys().cloned().collect();
        codes.sort();
        for code in &s.watchlist {
            if !codes.contains(code) {
                codes.push(code.clone());
            }
        }
        codes
    };

    // Drop monitoring rows for symbols no longer watched/held.
    {
        let mut s = state.lock().await;
        s.monitor.retain(|c, _| codes.contains(c));
    }

    let eod = { eod_phase(&*state.lock().await) };

    for code in codes {
        // Each symbol trades on its assigned strategy's recommended timeframe.
        let tf = { state.lock().await.tf_for(&code) };
        // Bar-close scheduling: a non-held symbol only needs a fetch once its
        // TF bar could have closed since the last one we processed. Skipping
        // the rest keeps the cycle inside the eBest rate-limit budget for
        // large watchlists (its previous monitor row simply stays in place).
        // Held symbols are always fetched — their stops track the live price.
        {
            let s = state.lock().await;
            let held = s.risk.position(&code).is_some();
            if !held {
                if let Some(&prev_ts) = s.last_bar_ts.get(&code) {
                    if kst_wallclock_unix() < prev_ts + tf_secs(tf) {
                        continue;
                    }
                }
            }
        }
        let candles = deps.fetcher.fetch(token, &code, tf).await;
        if candles.len() < 21 {
            let mut s = state.lock().await;
            s.record_monitor(&code, 0.0, 0.0, 0.0, Eval::new("데이터부족", "캔들 데이터 부족 (21봉 미만)"));
            continue;
        }
        // Last bar is still forming → its price is only used for hard stop / averaging.
        let mut live_price = candles.last().unwrap().close;
        let closed = &candles[..candles.len() - 1];
        if closed.len() < 20 {
            continue;
        }
        let atr = compute_atr(closed, 14);
        let bar_price = closed.last().unwrap().close;
        let bar_ts = candle_unix_ts(&closed[closed.len() - 1]);
        let live_ts = candle_unix_ts(candles.last().unwrap());

        // 봉 신선도 가드: 마지막 닫힌 봉이 너무 오래됐으면(장 마감 후·휴장일·피드
        // 장애) 그 봉의 종가는 지금 체결 가능한 가격이 아니다. 진입/청산/스탑을
        // 모두 건너뛰고 다음 사이클에 재평가한다 — `last_bar_ts` 도 갱신하지 않아
        // 신선한 봉이 돌아오면 정상적으로 새 봉으로 인식된다.
        let bar_age = kst_wallclock_unix() - bar_ts;
        if bar_age > stale_after_secs(tf) {
            let mut s = state.lock().await;
            s.record_monitor(&code, bar_price, live_price, atr, Eval::new(
                "데이터정지",
                format!("마지막 마감봉이 {}분 전 — 장외/휴장/피드지연으로 판단 보류", bar_age / 60),
            ));
            continue;
        }

        let holds = { state.lock().await.risk.position(&code).is_some() };
        // The intrabar hard stop advertises "실시간가" — but the candle cache can
        // be up to its TTL stale (35–90s). For held positions with the hard stop
        // armed, pull the true current price (t1101) so a crash is caught now.
        if holds {
            let hard = { state.lock().await.risk_for(&code).hard_stop_intrabar };
            if hard {
                if let Some(p) = fresh_price(deps, token, &code).await {
                    live_price = p;
                }
            }
        }
        let equity = {
            let mut s = state.lock().await;
            s.current_prices.insert(code.clone(), live_price);
            s.equity()
        };
        // How many closed bars arrived since the one we last processed. With a
        // large watchlist a cycle can take minutes, so several bars may have
        // closed — signals on those bars must still be seen (scan window),
        // and bar-counted state (cooldown, guards, bars_held) must advance by
        // the same amount instead of one per cycle.
        let missed: i64 = {
            let s = state.lock().await;
            match s.last_bar_ts.get(&code) {
                None => 1,
                Some(&prev_ts) => match closed.iter().rev().position(|c| candle_unix_ts(c) == prev_ts) {
                    Some(p) => (p as i64).min(MISSED_BARS_CAP),
                    None => MISSED_BARS_CAP, // last-seen bar rolled out of history — long gap
                },
            }
        };
        let new_bar = missed > 0;

        // EOD flatten: force-close held positions, no new entries near the close.
        if eod == EodPhase::Flatten {
            if holds {
                exit(deps, state, token, &code, live_price, "eod_flatten", live_ts).await;
            }
            let mut s = state.lock().await;
            if new_bar {
                s.last_bar_ts.insert(code.clone(), bar_ts);
            }
            s.record_monitor(&code, bar_price, live_price, atr,
                Eval::new("마감청산", "장 마감 전 강제 청산 시간 (15:10~) — 신규 진입 없음"));
            continue;
        }

        // 전일 이월 포지션(엔진 미가동으로 EOD 청산을 놓친 것)은 스탑 평가를
        // 기다리지 않고 장 재개 첫 사이클에 즉시 청산해 갭 손실 누적을 끊는다.
        // eod_flatten 이 꺼진 설정(스윙 허용)이면 표시만 지우고 정상 관리한다.
        if holds && { state.lock().await.flatten_stale.contains(&code) } {
            let do_flatten = { state.lock().await.risk.cfg.eod_flatten };
            if !do_flatten {
                state.lock().await.flatten_stale.remove(&code);
            } else {
                exit(deps, state, token, &code, live_price, "stale_flatten", live_ts).await;
                let mut s = state.lock().await;
                if new_bar {
                    s.last_bar_ts.insert(code.clone(), bar_ts);
                }
                let eval = if s.risk.position(&code).is_some() {
                    Eval::new("이월청산", "전일 이월 포지션 청산 주문 미체결 — 다음 사이클 재시도")
                } else {
                    s.flatten_stale.remove(&code);
                    Eval::new("이월청산", "전일 이월 포지션 — 장 재개 후 즉시 청산")
                };
                s.record_monitor(&code, bar_price, live_price, atr, eval);
                continue;
            }
        }
        if holds {
            manage_open(deps, state, token, &code, live_price, bar_price, atr, missed, live_ts, bar_ts).await;
            if new_bar {
                state.lock().await.last_bar_ts.insert(code.clone(), bar_ts);
            }
            // 보유 종목은 미실현 수익률을 모니터링에 표시 (이번 사이클에 청산됐으면 재평가 대기).
            {
                let mut s = state.lock().await;
                let eval = match s.risk.position(&code) {
                    Some(p) => {
                        let upct = if p.entry != 0.0 { (live_price - p.entry) / p.entry * 100.0 } else { 0.0 };
                        Eval::new("보유중", format!("포지션 보유 · 미실현 {upct:+.2}%"))
                    }
                    None => Eval::new("청산됨", "이번 사이클에 청산 · 다음 봉부터 재평가"),
                };
                s.record_monitor(&code, bar_price, live_price, atr, eval);
            }
            continue;
        }
        // Not held: only act on a freshly closed bar; decrement cooldown first.
        if !new_bar {
            let mut s = state.lock().await;
            s.record_monitor(&code, bar_price, live_price, atr,
                Eval::new("봉마감대기", "닫힌 봉 갱신 대기 (진입은 봉 마감 시 판단)"));
            continue;
        }
        {
            let mut s = state.lock().await;
            s.last_bar_ts.insert(code.clone(), bar_ts);
            let cd = s.cooldown.get(&code).copied().unwrap_or(0);
            if cd > 0 {
                // Advance by the bars actually elapsed, not one per cycle.
                let remaining = (cd - missed).max(0);
                s.cooldown.insert(code.clone(), remaining);
                if remaining > 0 {
                    s.record_monitor(&code, bar_price, live_price, atr,
                        Eval::new("쿨다운", format!("재진입 금지 {remaining}봉 남음")));
                    continue;
                }
            }
        }
        if eod == EodPhase::NoEntry {
            let mut s = state.lock().await;
            s.record_monitor(&code, bar_price, live_price, atr,
                Eval::new("마감임박", "장 마감 임박 (15:05~) — 신규 진입 중단"));
            continue;
        }
        let eval = try_enter(deps, state, token, &code, closed, bar_price, atr, equity, bar_ts, tf, missed).await;
        {
            let mut s = state.lock().await;
            s.record_monitor(&code, bar_price, live_price, atr, eval);
        }
    }
    Ok(())
}

/// Manage a held position: optional intrabar hard stop + closed-bar exits + averaging.
/// `new_bars` = closed bars elapsed since the last processed one (0 = none).
#[allow(clippy::too_many_arguments)]
async fn manage_open(
    deps: &Deps,
    state: &Arc<Mutex<State>>,
    token: &str,
    code: &str,
    live_price: f64,
    bar_price: f64,
    atr: f64,
    new_bars: i64,
    live_ts: i64,
    bar_ts: i64,
) {
    let new_bar = new_bars > 0;
    // Exits obey the *symbol's* strategy: a v2 position keeps its ATR stop and
    // intrabar evaluation even if the operator's form still holds legacy values.
    let (cfg, held_bars) = {
        let mut s = state.lock().await;
        if new_bar {
            let h = s.bars_held.get(code).copied().unwrap_or(0) + new_bars;
            s.bars_held.insert(code.to_string(), h);
        }
        (s.risk_for(code), s.bars_held.get(code).copied().unwrap_or(0))
    };

    // Averaging-down is only allowed while the daily loss limit (realized +
    // unrealized) is intact — 물타기 must not bypass the loss cap by adding to a
    // sinking position after the day's risk budget is spent.
    let may_average = |s: &State, code: &str| -> bool {
        s.risk.can_average(code) && s.risk.daily_loss_ok(s.equity())
    };

    // 1) Intrabar hard stop (off by default): protect on the forming bar's price.
    //    Only when the price stop-loss is enabled at all — `use_stop_loss` off
    //    disables every price-based stop, intrabar included.
    if cfg.hard_stop_intrabar && cfg.use_stop_loss {
        let hit = { state.lock().await.risk.hard_stop_hit(code, live_price, cfg.hard_stop_buffer_pct) };
        if hit {
            let can_avg = { let s = state.lock().await; may_average(&s, code) };
            if can_avg {
                fib_average_down(deps, state, token, code, live_price, atr, live_ts).await;
            } else {
                exit(deps, state, token, code, live_price, "stop_loss", live_ts).await;
            }
            return;
        }
    }
    // 2) Take-profit / trailing / stop are judged on closed bars only.
    if !new_bar {
        return;
    }
    let reason = { state.lock().await.risk.check_exit_with(code, bar_price, atr, &cfg) };
    let Some(reason) = reason else { return };
    // min_hold: defer only *trailing* exits. Stops must always fire, and a hit
    // profit target is banked immediately — deferring it just hands the gain
    // back when the price mean-reverts before min_hold elapses.
    if reason == "trailing_stop" && held_bars < cfg.min_hold_bars {
        return;
    }
    if reason == "stop_loss" {
        let can_avg = { let s = state.lock().await; may_average(&s, code) };
        if can_avg {
            fib_average_down(deps, state, token, code, bar_price, atr, bar_ts).await;
            return;
        }
    }
    exit(deps, state, token, code, bar_price, reason, bar_ts).await;
}

/// On a stop signal, buy a Fibonacci-sized lot instead of selling to lower the average.
async fn fib_average_down(deps: &Deps, state: &Arc<Mutex<State>>, token: &str, code: &str, price: f64, atr: f64, candle_ts: i64) {
    let (mut add_qty, order_type, max_buy, cash, entered) = {
        let s = state.lock().await;
        (s.risk.next_fib_qty(code), s.order.order_type.clone(), s.order.max_buy_amount, s.cash, s.risk.total_entered_amount())
    };
    // max_buy_amount caps the *total* entered amount across all positions, so
    // the room left for this add is the cap minus what's already committed.
    if max_buy > 0.0 && price > 0.0 {
        let remaining = (max_buy - entered).max(0.0);
        add_qty = add_qty.min((remaining / price) as i64);
    }
    if price > 0.0 {
        add_qty = add_qty.min((cash / price) as i64);
    }
    if add_qty <= 0 {
        exit(deps, state, token, code, price, "stop_loss", candle_ts).await; // out of cash → stop out
        return;
    }
    let fill = deps.broker.buy(token, code, add_qty, price, &order_type).await;
    if !fill.ok || fill.qty <= 0 {
        return;
    }
    let added = fill.qty.min(add_qty); // 지정가 부분 체결 대비
    let mut s = state.lock().await;
    s.cash -= added as f64 * fill.fill_price;
    let eff = s.risk_for(code);
    let pos = s.risk.average_down(code, fill.fill_price, added, atr, &eff);
    let lvl = pos.as_ref().map_or(0, |p| p.fib_level);
    s.persist(deps.broker.mode);
    s.append_event(json!({
        "code": code, "name": name_for(code), "type": "buy", "price": fill.fill_price,
        "qty": added, "pnl": 0.0, "pnl_pct": 0.0, "reason": format!("fib_avg_down_{lvl}"),
        "ts": candle_ts, "time_label": hhmm_label(),
    }));
    tracing::info!("FIB AVG DOWN {code} lvl={lvl} x{added} @{:.0}", fill.fill_price);
    notify(deps, format!(
        "💧 [{}] 물타기 {lvl}차 — {}({code}) +{added}주 @{:.0} (평단 {:.0})",
        mode_label(deps.broker.mode), name_for(code), fill.fill_price,
        pos.as_ref().map_or(0.0, |p| p.entry)
    ));
}

/// Close (sell) a position, then arm the cooldown / re-buy price guard.
async fn exit(deps: &Deps, state: &Arc<Mutex<State>>, token: &str, code: &str, price: f64, reason: &str, candle_ts: i64) {
    let (pos, sell_qty, order_type) = {
        let mut s = state.lock().await;
        // In-flight guard: the broker call below runs outside the state lock
        // (a real HTTP round trip in live mode), so a concurrent manual close
        // could otherwise sell the same position twice.
        if s.closing.contains(code) {
            return;
        }
        let Some(pos) = s.risk.position(code).cloned() else { return };
        // Stop-outs / EOD / manual closes always exit in full — partial-selling
        // into a falling stop leaves the remainder exposed with no protection.
        let force_full = matches!(reason, "stop_loss" | "eod_flatten" | "stale_flatten" | "manual_close");
        let sell_qty = if force_full || s.order.sell_all || s.order.fixed_qty.is_none() {
            pos.qty
        } else {
            s.order.fixed_qty.unwrap().min(pos.qty)
        };
        s.closing.insert(code.to_string());
        (pos, sell_qty, s.order.order_type.clone())
    };
    let fill = deps.broker.sell(token, code, sell_qty, price, &order_type).await;
    if !fill.ok || fill.qty <= 0 {
        state.lock().await.closing.remove(code);
        return; // 미체결/실패 — 포지션 유지, 다음 사이클에 청산 조건 재평가
    }
    // 지정가 부분 체결 대비: 실제 체결수량 기준으로 정산한다.
    let sold = fill.qty.min(sell_qty);
    let mut s = state.lock().await;
    s.closing.remove(code);
    let pnl = (fill.fill_price - pos.entry) * sold as f64;
    s.cash += sold as f64 * fill.fill_price;
    let pnl_pct = if pos.entry != 0.0 { (fill.fill_price - pos.entry) / pos.entry * 100.0 } else { 0.0 };
    let closed = s.risk.reduce(code, sold, pnl);
    record_close(deps, &mut s, code, &pos, fill.fill_price, reason, sold, closed);
    s.append_event(json!({
        "code": code, "name": name_for(code),
        "type": "sell", "action": "close",
        "price": fill.fill_price,
        "qty": sold, "pnl": pnl.round(), "pnl_pct": (pnl_pct * 100.0).round() / 100.0,
        "reason": reason, "ts": candle_ts, "time_label": hhmm_label(),
    }));
    tracing::info!("EXIT {code} ({reason}) x{sold} pnl={:.0}{}", pnl, if closed { "" } else { " (부분청산)" });
    notify(deps, format!(
        "🔴 [{}] 청산 — {}({code}) {sold}주 @{:.0}\n손익 {:+.0}원 ({pnl_pct:+.2}%) · 사유 {reason}{}",
        mode_label(deps.broker.mode), name_for(code), fill.fill_price, pnl,
        if closed { "" } else { " · 부분청산" }
    ));
    if closed {
        // Re-entry guard: stop-outs cool down longer + block re-buys above the exit price.
        s.bars_held.remove(code);
        if reason == "stop_loss" {
            let cd = s.risk.cfg.loss_cooldown_bars;
            s.cooldown.insert(code.to_string(), cd);
            // Price guard with a bar-count expiry (0 = never expires) so a
            // recovered uptrend isn't blocked from re-entry forever.
            let guard_bars = s.risk.cfg.reentry_guard_expire_bars.max(0);
            s.last_exit_price.insert(code.to_string(), (fill.fill_price, guard_bars));
        } else {
            let cd = s.risk.cfg.reentry_cooldown_bars;
            s.cooldown.insert(code.to_string(), cd);
            s.last_exit_price.remove(code);
        }
    }
    s.persist(deps.broker.mode);
}

/// Find the bar index (in `closed`) a pattern was detected on — its last used candle.
fn pattern_bar_index(closed: &[Candle], r: &PatternResult) -> Option<usize> {
    let last = r.candles_used.last()?;
    closed.iter().rposition(|c| c.ts == last.ts)
}

/// Entry decision: price guard → signal (closed bar) → confirm → higher-TF trend → MTF → gates → size.
/// `missed` = closed bars elapsed since the previous evaluation; the pattern
/// scan covers all of them so slow cycles don't drop signals on skipped bars.
#[allow(clippy::too_many_arguments)]
async fn try_enter(
    deps: &Deps,
    state: &Arc<Mutex<State>>,
    token: &str,
    code: &str,
    closed: &[Candle],
    price: f64,
    atr: f64,
    equity: f64,
    candle_ts: i64,
    tf: Timeframe,
    missed: i64,
) -> Eval {
    let (cfg, strategy) = {
        let s = state.lock().await;
        (s.risk_for(code), s.strategy_for(code))
    };

    // Re-buy price guard — don't buy back at/above the stop-out price. The
    // guard expires after `reentry_guard_expire_bars` closed bars (0 = never),
    // so a recovered uptrend eventually becomes enterable again.
    {
        let mut s = state.lock().await;
        if let Some((guard, bars_left)) = s.last_exit_price.get(code).copied() {
            if price < guard * (1.0 - cfg.reentry_gap_pct) {
                s.last_exit_price.remove(code); // price fell below the guard → re-buy allowed
            } else if bars_left == 0 {
                // 0 = no expiry — block until the price drops below the guard.
                return Eval::new("재매수가드", format!("직전 손절가 {:.0} 위 → 재매수 보류 (무기한)", guard));
            } else {
                // Advance by the bars actually elapsed since the last evaluation.
                let remaining = bars_left - missed.max(1);
                if remaining <= 0 {
                    s.last_exit_price.remove(code); // guard expired
                } else {
                    s.last_exit_price.insert(code.to_string(), (guard, remaining));
                    return Eval::new("재매수가드",
                        format!("직전 손절가 {:.0} 위 → 재매수 보류 ({remaining}봉 후 해제)", guard));
                }
            }
        }
    }

    if closed.len() < 13 {
        return Eval::new("데이터부족", "패턴 분석 데이터 부족");
    }

    // Scan window: every bar closed since the last evaluation (missed) plus,
    // when confirmation is on, `confirm_window_bars` older bars whose pattern
    // may only now be getting its confirming bar.
    let win_missed = missed.clamp(1, MISSED_BARS_CAP) as usize;
    let confirm_win = cfg.confirm_window_bars.max(1) as usize;
    let scan_window = if cfg.require_confirmation { win_missed + confirm_win } else { win_missed };

    // Candlestick patterns (windowed) + context setups (VWAP/ORB/EMA over the
    // full session) form a single candidate pool, scored the same way.
    let mut candidates = deps.detector.scan(closed, tf, 0.0, true, &strategy, scan_window);
    {
        let ctx = SessionContext::for_tf(closed, tf);
        let series = SetupSeries::compute(closed);
        for k in 0..scan_window.min(closed.len()) {
            let i = closed.len() - 1 - k;
            let mut setups = detect_setups(closed, i, &ctx, &series, &strategy.enabled_patterns);
            setups.extend(detect_v2_setups(closed, i, &ctx, &series, &strategy.enabled_patterns));
            for s in &mut setups {
                apply_strategy(s, &strategy, false, false, false);
            }
            candidates.append(&mut setups);
        }
    }
    // Keep only the patterns this strategy has enabled.
    candidates.retain(|r| strategy.enabled_patterns.contains(&r.pattern_name));
    candidates.sort_by(|a, b| b.composite_score.partial_cmp(&a.composite_score).unwrap_or(std::cmp::Ordering::Equal));
    if candidates.is_empty() {
        return Eval::new("신호없음", "매매 패턴 미감지");
    }

    // Pick the best *actionable* candidate. A candidate fires only once:
    // - no confirmation: its pattern bar must be one of the newly closed bars
    // - confirmation: a confirming bar must exist within `confirm_win` bars of
    //   the pattern, and that confirming bar must itself be newly closed
    //   (older pattern+confirm pairs were already evaluated in a past cycle).
    let last_idx = closed.len() - 1;
    let new_cut = closed.len() - win_missed.min(closed.len()); // indexes >= new_cut are new
    let mut pending: Option<(f64, String)> = None; // best new-but-unconfirmed signal
    let mut chosen: Option<PatternResult> = None;
    for cand in candidates {
        let Some(pi) = pattern_bar_index(closed, &cand) else { continue };
        if !cfg.require_confirmation {
            if pi >= new_cut {
                chosen = Some(cand);
                break;
            }
            continue;
        }
        let p_high = cand.candles_used.iter().map(|c| c.high).fold(f64::MIN, f64::max);
        let p_close = cand.candles_used.last().map(|c| c.close).unwrap_or(0.0);
        let hi = (pi + confirm_win).min(last_idx);
        let confirmed = (pi + 1..=hi).any(|j| {
            if j < new_cut {
                return false; // this confirmation already fired in a past cycle
            }
            let b = &closed[j];
            b.close > p_high || (b.is_bull() && b.close > p_close)
        });
        if confirmed {
            chosen = Some(cand);
            break;
        }
        if pending.is_none() && pi + confirm_win > last_idx {
            // still within its confirmation window — worth reporting as waiting
            pending = Some((cand.composite_score, cand.pattern_name.clone()));
        }
    }
    let Some(mut top) = chosen else {
        return match pending {
            Some((score, name)) => Eval::with_signal(
                "확인봉대기", format!("{name} 감지 · 확인봉 대기 (패턴 후 {confirm_win}봉 내)"), score, &name),
            None => Eval::new("신호없음", "매매 패턴 미감지 (새 봉 기준)"),
        };
    };

    // Market-context filter (v2 strategies) — the broad market must be risk-on
    // and the symbol must be leading it. Nothing in the legacy path looked at
    // the market at all, which is how a rising index still produced losses:
    // entries kept landing in names lagging that rise.
    if strategy.market_filter.enabled {
        let mf = &strategy.market_filter;
        let mkt = market::assess(&deps.fetcher, token, &mf.proxy_code, tf, mf.rs_lookback).await;
        {
            let mut s = state.lock().await;
            s.market = Some(mkt.clone());
        }
        if mf.require_risk_on && !mkt.risk_on {
            let why = if mkt.above_ema { "EMA20 하락 전환" } else { "EMA20 이탈" };
            return Eval::with_signal(
                "시장위험",
                format!("{} 감지 · 시장({}) 리스크오프 — {why}", top.pattern_name, mf.proxy_code),
                top.composite_score,
                &top.pattern_name,
            );
        }
        let rs = market::relative_strength(closed, &mkt, mf.rs_lookback);
        if rs < mf.min_rs {
            return Eval::with_signal(
                "상대약세",
                format!(
                    "{} 감지 · 시장 대비 {rs:+.2}%p (기준 {:+.2}%p) — 주도주 아님",
                    top.pattern_name, mf.min_rs
                ),
                top.composite_score,
                &top.pattern_name,
            );
        }
    }

    // Higher-TF trend filter — block trades fighting the upper timeframe.
    // `higher_tf_slope_tolerance` relaxes the gate to "block only on a clear
    // downtrend" instead of any negative drift.
    if cfg.require_higher_tf_uptrend {
        let mtf = MtfEngine::new(&deps.fetcher, &deps.detector);
        if !mtf.higher_tf_uptrend(token, code, tf, cfg.higher_tf_slope_tolerance).await {
            tracing::info!("entry blocked {code}: higher_tf downtrend");
            return Eval::with_signal("상위TF역행", format!("{} 감지 · 상위 TF 역행으로 진입 차단", top.pattern_name), top.composite_score, &top.pattern_name);
        }
    }

    // MTF confluence score → recompute composite.
    {
        let mtf = MtfEngine::new(&deps.fetcher, &deps.detector);
        top.mtf_score = mtf.score(token, code, tf).await;
        apply_strategy(&mut top, &strategy, true, false, false);
    }

    // Cost/noise entry gates + risk gate.
    let (gate_ok, why) = passes_entry_gates(&strategy, &cfg, &deps.cost, &top, price, atr);
    if !gate_ok {
        tracing::info!("entry gated {code}: {why}");
        return Eval::with_signal("게이트미달", format!("{} 감지 · 게이트 미달 ({why})", top.pattern_name), top.composite_score, &top.pattern_name);
    }
    let (can, why) = {
        let s = state.lock().await;
        let (c, w) = s.risk.can_enter_with(equity, &cfg);
        (c, w.to_string())
    };
    if !can {
        tracing::info!("entry blocked {code}: {why}");
        // 일손실 한도 도달은 하루 1회 텔레그램으로 즉시 알린다 — 이후 이 날의
        // 모든 신규 진입이 중단되는 중요 이벤트이므로 지켜보지 않아도 알 수 있게.
        if why == "daily_loss_limit_reached" {
            let mut s = state.lock().await;
            if !s.loss_limit_notified {
                s.loss_limit_notified = true;
                let daily = s.risk.daily_pnl();
                drop(s);
                notify(deps, format!(
                    "🚨 [{}] 일손실 한도 도달 — 오늘 신규 진입 중단\n일손익 {daily:+.0}원",
                    mode_label(deps.broker.mode)
                ));
            }
        }
        return Eval::with_signal("진입제한", format!("{} 감지 · 리스크 한도로 진입 제한 ({why})", top.pattern_name), top.composite_score, &top.pattern_name);
    }

    // Sizing: fixed qty or risk-based, capped by the portfolio-wide max buy
    // amount (total entered across all positions, not just this order) and cash.
    let (mut qty, order_type, max_buy, cash, entered) = {
        let s = state.lock().await;
        let q = s.order.fixed_qty.unwrap_or_else(|| s.risk.position_size_with(equity, price, atr, &cfg));
        (q, s.order.order_type.clone(), s.order.max_buy_amount, s.cash, s.risk.total_entered_amount())
    };
    if max_buy > 0.0 && price > 0.0 {
        let remaining = (max_buy - entered).max(0.0);
        qty = qty.min((remaining / price) as i64);
    }
    if price > 0.0 {
        qty = qty.min((cash / price) as i64);
    }
    if qty <= 0 {
        return Eval::with_signal("수량부족", format!("{} 감지 · 가용현금/한도로 매수수량 0", top.pattern_name), top.composite_score, &top.pattern_name);
    }
    let fill = deps.broker.buy(token, code, qty, price, &order_type).await;
    if !fill.ok || fill.qty <= 0 {
        return Eval::with_signal("주문실패", format!("{} 진입 주문 실패(미체결)", top.pattern_name), top.composite_score, &top.pattern_name);
    }
    let filled = fill.qty.min(qty); // 지정가 부분 체결 대비: 실제 체결수량만 등록
    let mut s = state.lock().await;
    s.cash -= filled as f64 * fill.fill_price;
    let (stop, target) = s.risk.stop_and_target_with(fill.fill_price, atr, &cfg);
    s.risk.register(code, fill.fill_price, filled, stop, target);
    s.opened_meta.insert(code.to_string(), (now_iso(), top.pattern_name.clone()));
    s.bars_held.insert(code.to_string(), 0);
    s.last_exit_price.remove(code);
    s.persist(deps.broker.mode);
    s.append_event(json!({
        "code": code, "name": name_for(code),
        "type": "buy", "action": "open",
        "price": fill.fill_price,
        "qty": filled, "pnl": 0.0, "pnl_pct": 0.0, "reason": top.pattern_name,
        "ts": candle_ts, "time_label": hhmm_label(),
    }));
    tracing::info!("ENTER {code} x{filled} @{:.0} stop={:.0} target={:.0} ({} {:.2})",
        fill.fill_price, stop, target, top.pattern_name, top.composite_score);
    notify(deps, format!(
        "🟢 [{}] 진입 — {}({code}) 매수 {filled}주 @{:.0}\n{} · 점수 {:.2} · 손절 {:.0} · 익절 {:.0}",
        mode_label(deps.broker.mode), name_for(code), fill.fill_price,
        top.pattern_name, top.composite_score, stop, target
    ));
    Eval::with_signal("진입", format!("{} 매수 {filled}주 @{:.0}", top.pattern_name, fill.fill_price), top.composite_score, &top.pattern_name)
}

/// Cost/noise entry gates (composite threshold + volume + reward/risk + edge-over-cost).
fn passes_entry_gates(
    strategy: &StrategyConfig,
    risk: &RiskConfig,
    cost: &CostModel,
    result: &PatternResult,
    price: f64,
    atr: f64,
) -> (bool, String) {
    if result.composite_score < strategy.entry_threshold {
        return (false, format!("composite<{}", strategy.entry_threshold));
    }
    if strategy.require_volume_confirm && !result.volume_confirmed {
        return (false, "volume_not_confirmed".into());
    }
    // Use the *actual* stop/target distances (manual fixed-% takes priority over
    // ATR multiples) so the gates judge the same exits the engine will place.
    let (stop_dist, target_dist) = risk.stop_target_dists(price, atr);
    if strategy.min_reward_risk > 0.0 && stop_dist > 0.0 {
        let rr = target_dist / stop_dist;
        if rr < strategy.min_reward_risk {
            return (false, format!("reward_risk {rr:.2}<{}", strategy.min_reward_risk));
        }
    }
    if strategy.min_edge_over_cost > 0.0 && price > 0.0 {
        let target_move_pct = target_dist / price * 100.0;
        let cost_pct = cost.round_trip_cost() * 100.0;
        if target_move_pct < cost_pct * strategy.min_edge_over_cost {
            return (false, format!("edge {target_move_pct:.2}%<{:.2}%", cost_pct * strategy.min_edge_over_cost));
        }
    }
    (true, "ok".into())
}

/// Journal a (full or partial) close.
fn record_close(deps: &Deps, s: &mut State, code: &str, pos: &crate::risk::Position, exit_price: f64, reason: &str, qty: i64, fully_closed: bool) {
    let meta = if fully_closed {
        s.opened_meta.remove(code)
    } else {
        s.opened_meta.get(code).cloned()
    };
    let (opened_at, pattern) = meta.unwrap_or_default();
    let ret_pct = if pos.entry != 0.0 { (exit_price - pos.entry) / pos.entry * 100.0 } else { 0.0 };
    deps.journal.record(&TradeRecord {
        mode: deps.broker.mode.as_str().to_string(),
        code: code.to_string(),
        qty,
        entry: pos.entry,
        exit: exit_price,
        pnl: (exit_price - pos.entry) * qty as f64,
        return_pct: ret_pct,
        reason: reason.to_string(),
        pattern,
        opened_at,
        closed_at: now_iso(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_guard_tolerates_normal_bar_lag_but_catches_session_gaps() {
        // 10분봉: 정상 지연(마감 직후 ~ 다음 봉 형성 중)은 통과해야 한다.
        let limit = stale_after_secs(Timeframe::M10);
        assert!(limit > tf_secs(Timeframe::M10) * 2, "정상 봉 지연을 오탐하면 안 된다");
        // 하지만 하룻밤(17시간)은 반드시 걸러야 한다.
        assert!(17 * 3600 > limit, "장 마감 후 스테일 봉은 차단되어야 한다");
        // 1분봉도 마찬가지 — 주말(2일)은 확실히 차단.
        assert!(2 * 86_400 > stale_after_secs(Timeframe::M1));
    }

    #[test]
    fn daily_timeframe_survives_a_weekend() {
        // 일봉은 금요일 봉을 월요일에 평가해도 유효해야 한다 (3일 경과).
        assert!(stale_after_secs(Timeframe::D1) > 3 * 86_400);
    }

    #[test]
    fn snapshot_roundtrips_last_bar_ts() {
        // 재시작 후 이미 처리한 봉을 다시 거래하지 않으려면 왕복 보존이 필수.
        let snap = Snapshot {
            day: "2026-08-04".into(),
            cash: 1.0,
            seed_cash: 1.0,
            daily_pnl: 0.0,
            day_start_equity: 1.0,
            positions: HashMap::new(),
            opened_meta: HashMap::new(),
            cooldown: HashMap::new(),
            last_exit_price: HashMap::new(),
            bars_held: HashMap::new(),
            last_bar_ts: HashMap::from([("005930".to_string(), 1_754_000_000i64)]),
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.last_bar_ts.get("005930"), Some(&1_754_000_000));
    }

    #[test]
    fn legacy_snapshot_without_last_bar_ts_still_loads() {
        let legacy = r#"{"day":"2026-08-04","cash":1.0,"seed_cash":1.0,"daily_pnl":0.0,
            "day_start_equity":1.0,"positions":{},"opened_meta":{},"cooldown":{},
            "last_exit_price":{},"bars_held":{}}"#;
        let snap: Snapshot = serde_json::from_str(legacy).expect("구버전 스냅샷 호환");
        assert!(snap.last_bar_ts.is_empty());
    }
}
