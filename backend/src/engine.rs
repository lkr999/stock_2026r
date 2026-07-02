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
use crate::indicators::ema_values;
use crate::journal::{now_iso, TradeJournal, TradeRecord};
use crate::mtf::MtfEngine;
use crate::pattern::{apply_strategy, compute_atr, detect_setups, PatternDetector, PatternResult};
use crate::risk::{RiskConfig, RiskManager, Side};
use crate::session::SessionContext;
use crate::strategy::{Source, StrategyConfig};
use crate::timeframe::Timeframe;
use crate::universe::name_for;
use chrono::{Datelike, NaiveDate, Timelike, Utc, Weekday};
use chrono_tz::Asia::Seoul;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Order-execution settings (per-order qty, type, sell-all, portfolio buy cap).
#[derive(Clone)]
pub struct OrderConfig {
    pub order_type: String,
    pub fixed_qty: Option<i64>,
    pub sell_all: bool,
    /// Cap on **total entered amount** across all open positions (Σ entry×qty,
    /// both longs and shorts) — not a per-order limit. New entries and
    /// averaging-down adds are sized down so the running total never exceeds
    /// this (0 = uncapped).
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
    last_exit_price: HashMap<String, f64>,
    bars_held: HashMap<String, i64>,
    current_prices: HashMap<String, f64>,
    trade_events: Vec<Value>,
    monitor: HashMap<String, Value>, // code -> latest per-cycle monitoring snapshot
}

impl State {
    /// Equity = cash + position marks. Longs are valued at market; short opens
    /// credit no cash, so they contribute only their unrealized P&L (entry−price).
    fn equity(&self) -> f64 {
        let held: f64 = self
            .risk
            .open_positions()
            .iter()
            .map(|(code, p)| {
                let cur = self.current_prices.get(code).copied().unwrap_or(p.entry);
                match p.side {
                    Side::Long => p.qty as f64 * cur,
                    Side::Short => (p.entry - cur) * p.qty as f64,
                }
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
    ) -> Self {
        let mode = broker.mode;
        let deps = Arc::new(Deps { ebest, fetcher, detector, broker, journal, cost: CostModel::default() });
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
            trade_events: vec![],
            monitor: HashMap::new(),
        }));
        Self { deps, state, mode, task: std::sync::Mutex::new(None) }
    }

    pub fn mode(&self) -> TradingMode {
        self.mode
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
        let s = self.state.lock().await;
        let Some(pos) = s.risk.position(code).cloned() else {
            return json!({"ok": false, "reason": "no_position"});
        };
        let (qty, entry, side) = (pos.qty, pos.entry, pos.side);
        let order_type = s.order.order_type.clone();
        drop(s);
        let fill = match side {
            Side::Long => self.deps.broker.sell(&token, code, qty, price, &order_type).await,
            Side::Short => self.deps.broker.cover(&token, code, qty, price, &order_type).await,
        };
        if !fill.ok {
            return json!({"ok": false, "reason": "order_failed"});
        }
        let mut s = self.state.lock().await;
        let dir = if side == Side::Short { -1.0 } else { 1.0 };
        let pnl = (fill.fill_price - entry) * qty as f64 * dir;
        match side {
            Side::Long => s.cash += qty as f64 * fill.fill_price,
            Side::Short => s.cash += pnl,
        }
        let pnl_pct = if entry != 0.0 { (fill.fill_price - entry) / entry * 100.0 * dir } else { 0.0 };
        record_close(&self.deps, &mut s, code, &pos, fill.fill_price, "manual_close", qty, true);
        s.risk.reduce(code, qty, pnl);
        s.append_event(json!({
            "code": code, "name": name_for(code), "side": side.as_str(),
            "type": if side == Side::Short { "buy" } else { "sell" }, "action": "close",
            "price": fill.fill_price,
            "qty": qty, "pnl": pnl.round(), "pnl_pct": (pnl_pct * 100.0).round() / 100.0,
            "reason": "manual_close", "ts": candle_ts, "time_label": hhmm_label(),
        }));
        tracing::info!("MANUAL CLOSE {code} x{qty} @{:.0} pnl={:.0}", fill.fill_price, pnl);
        json!({"ok": true, "fill_price": fill.fill_price, "qty": qty,
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
                let dir = if p.side == Side::Short { -1.0 } else { 1.0 };
                let upnl = (cur - p.entry) * p.qty as f64 * dir;
                let upct = if p.entry != 0.0 { (cur - p.entry) / p.entry * 100.0 * dir } else { 0.0 };
                let meta = s.opened_meta.get(code);
                json!({
                    "code": code, "name": name_for(code), "side": p.side.as_str(),
                    "entry": p.entry, "qty": p.qty, "stop": p.stop, "target": p.target,
                    "peak": p.peak, "base_qty": p.base_qty, "fib_level": p.fib_level,
                    "buy_price": p.entry.round(), "current_price": cur.round(),
                    "unrealized_pnl": upnl.round(), "unrealized_pct": (upct * 100.0).round() / 100.0,
                    "pattern": meta.map(|m| m.1.clone()).unwrap_or_default(),
                    "opened_at": meta.map(|m| m.0.clone()).unwrap_or_default(),
                    "strategy": s.strategy_for(code).name, "tf": s.tf_for(code).as_str(),
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
        // Per-symbol strategy assignments (code → {strategy, tf}) for the UI.
        let symbol_strategies: serde_json::Map<String, Value> = s
            .symbol_strategies
            .iter()
            .map(|(code, cfg)| (code.clone(), json!({"strategy": cfg.name, "tf": cfg.recommended_tf().as_str()})))
            .collect();
        json!({
            "running": s.running,
            "mode": self.mode.as_str(),
            "strategy": s.strategy.name,
            "timeframe": s.tf.as_str(),
            "symbol_strategies": symbol_strategies,
            "watchlist": s.watchlist,
            "poll_sec": s.poll_sec,
            "ignore_market_hours": s.ignore_market_hours,
            "seed_cash": s.seed_cash.round(),
            "entry_threshold": s.strategy.entry_threshold,
            "weights": weights,
            "direction": s.strategy.direction,
            "cycles": s.cycles,
            "cash": s.cash.round(),
            "equity": s.equity().round(),
            "daily_pnl": s.risk.daily_pnl().round(),
            "positions": positions,
            "trade_events": recent,
            "monitor": monitor,
            "order": s.order.to_json(),
            "risk": s.risk.cfg.to_json(),
            "last_error": s.last_error,
        })
    }
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
        s.risk.reset_daily();
        s.cash = seed_equity(&deps, &token, s.seed_cash).await;
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
        let today = Utc::now().with_timezone(&Seoul).date_naive();
        if today != last_day {
            last_day = today;
            state.lock().await.risk.reset_daily();
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
        }
        // Sleep in 1s slices so a stop request takes effect promptly instead of
        // lingering for up to a full poll interval.
        let mut slept = 0u64;
        while slept < poll_sec.max(1) {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            slept += 1;
            if !state.lock().await.running {
                return;
            }
        }
    }
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

/// One full cycle over the watchlist + held codes.
async fn scan_and_trade(deps: &Deps, state: &Arc<Mutex<State>>, token: &str) -> Result<(), String> {
    // Snapshot codes under the lock (timeframe is resolved per symbol below).
    let codes = {
        let s = state.lock().await;
        let mut codes: Vec<String> = s.watchlist.clone();
        for code in s.risk.open_positions().keys() {
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

    for code in codes {
        // Each symbol trades on its assigned strategy's recommended timeframe.
        let tf = { state.lock().await.tf_for(&code) };
        let candles = deps.fetcher.fetch(token, &code, tf).await;
        if candles.len() < 21 {
            let mut s = state.lock().await;
            s.record_monitor(&code, 0.0, 0.0, 0.0, Eval::new("데이터부족", "캔들 데이터 부족 (21봉 미만)"));
            continue;
        }
        // Last bar is still forming → its price is only used for hard stop / averaging.
        let live_price = candles.last().unwrap().close;
        let closed = &candles[..candles.len() - 1];
        if closed.len() < 20 {
            continue;
        }
        let atr = compute_atr(closed, 14);
        let bar_price = closed.last().unwrap().close;
        let bar_ts = candle_unix_ts(&closed[closed.len() - 1]);
        let live_ts = candle_unix_ts(candles.last().unwrap());

        let equity = {
            let mut s = state.lock().await;
            s.current_prices.insert(code.clone(), live_price);
            s.equity()
        };
        let new_bar = { state.lock().await.last_bar_ts.get(&code).copied() != Some(bar_ts) };
        let holds = { state.lock().await.risk.position(&code).is_some() };

        if holds {
            manage_open(deps, state, token, &code, live_price, bar_price, atr, new_bar, live_ts, bar_ts).await;
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
                s.cooldown.insert(code.clone(), cd - 1);
                s.record_monitor(&code, bar_price, live_price, atr,
                    Eval::new("쿨다운", format!("재진입 금지 {cd}봉 남음")));
                continue;
            }
        }
        let eval = try_enter(deps, state, token, &code, closed, bar_price, atr, equity, bar_ts, tf).await;
        {
            let mut s = state.lock().await;
            s.record_monitor(&code, bar_price, live_price, atr, eval);
        }
    }
    Ok(())
}

/// Manage a held position: optional intrabar hard stop + closed-bar exits + averaging.
#[allow(clippy::too_many_arguments)]
async fn manage_open(
    deps: &Deps,
    state: &Arc<Mutex<State>>,
    token: &str,
    code: &str,
    live_price: f64,
    bar_price: f64,
    atr: f64,
    new_bar: bool,
    live_ts: i64,
    bar_ts: i64,
) {
    let (cfg, held_bars) = {
        let mut s = state.lock().await;
        if new_bar {
            let h = s.bars_held.get(code).copied().unwrap_or(0) + 1;
            s.bars_held.insert(code.to_string(), h);
        }
        (s.risk.cfg.clone(), s.bars_held.get(code).copied().unwrap_or(0))
    };

    // 1) Intrabar hard stop (off by default): protect on the forming bar's price.
    if cfg.hard_stop_intrabar {
        let hit = { state.lock().await.risk.hard_stop_hit(code, live_price, cfg.hard_stop_buffer_pct) };
        if hit {
            let can_avg = { state.lock().await.risk.can_average(code) };
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
    let reason = { state.lock().await.risk.check_exit(code, bar_price, atr) };
    let Some(reason) = reason else { return };
    // min_hold: defer profit/trailing exits until min_hold_bars (stops always allowed).
    if (reason == "take_profit" || reason == "trailing_stop") && held_bars < cfg.min_hold_bars {
        return;
    }
    if reason == "stop_loss" {
        let can_avg = { state.lock().await.risk.can_average(code) };
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
    if !fill.ok {
        return;
    }
    let mut s = state.lock().await;
    s.cash -= add_qty as f64 * fill.fill_price;
    let pos = s.risk.average_down(code, fill.fill_price, add_qty, atr);
    let lvl = pos.as_ref().map_or(0, |p| p.fib_level);
    s.append_event(json!({
        "code": code, "name": name_for(code), "type": "buy", "price": fill.fill_price,
        "qty": add_qty, "pnl": 0.0, "pnl_pct": 0.0, "reason": format!("fib_avg_down_{lvl}"),
        "ts": candle_ts, "time_label": hhmm_label(),
    }));
    tracing::info!("FIB AVG DOWN {code} lvl={lvl} x{add_qty} @{:.0}", fill.fill_price);
}

/// Close (sell) a position, then arm the cooldown / re-buy price guard.
async fn exit(deps: &Deps, state: &Arc<Mutex<State>>, token: &str, code: &str, price: f64, reason: &str, candle_ts: i64) {
    let (pos, sell_qty, order_type) = {
        let s = state.lock().await;
        let Some(pos) = s.risk.position(code).cloned() else { return };
        let sell_qty = if s.order.sell_all || s.order.fixed_qty.is_none() {
            pos.qty
        } else {
            s.order.fixed_qty.unwrap().min(pos.qty)
        };
        (pos, sell_qty, s.order.order_type.clone())
    };
    let fill = match pos.side {
        Side::Long => deps.broker.sell(token, code, sell_qty, price, &order_type).await,
        Side::Short => deps.broker.cover(token, code, sell_qty, price, &order_type).await,
    };
    if !fill.ok {
        return;
    }
    let mut s = state.lock().await;
    let dir = if pos.side == Side::Short { -1.0 } else { 1.0 };
    let pnl = (fill.fill_price - pos.entry) * sell_qty as f64 * dir;
    match pos.side {
        Side::Long => s.cash += sell_qty as f64 * fill.fill_price,
        Side::Short => s.cash += pnl, // short open credited no cash; realize P&L on cover
    }
    let pnl_pct = if pos.entry != 0.0 { (fill.fill_price - pos.entry) / pos.entry * 100.0 * dir } else { 0.0 };
    let closed = s.risk.reduce(code, sell_qty, pnl);
    record_close(deps, &mut s, code, &pos, fill.fill_price, reason, sell_qty, closed);
    s.append_event(json!({
        "code": code, "name": name_for(code), "side": pos.side.as_str(),
        "type": if pos.side == Side::Short { "buy" } else { "sell" }, "action": "close",
        "price": fill.fill_price,
        "qty": sell_qty, "pnl": pnl.round(), "pnl_pct": (pnl_pct * 100.0).round() / 100.0,
        "reason": reason, "ts": candle_ts, "time_label": hhmm_label(),
    }));
    tracing::info!("EXIT {code} ({reason}) x{sell_qty} pnl={:.0}{}", pnl, if closed { "" } else { " (부분청산)" });
    if closed {
        // Re-entry guard: stop-outs cool down longer + block re-buys above the exit price.
        s.bars_held.remove(code);
        if reason == "stop_loss" {
            let cd = s.risk.cfg.loss_cooldown_bars;
            s.cooldown.insert(code.to_string(), cd);
            s.last_exit_price.insert(code.to_string(), fill.fill_price);
        } else {
            let cd = s.risk.cfg.reentry_cooldown_bars;
            s.cooldown.insert(code.to_string(), cd);
            s.last_exit_price.remove(code);
        }
    }
}

/// Entry decision: price guard → signal (closed bar) → confirm → higher-TF trend → MTF → gates → size.
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
) -> Eval {
    let (cfg, strategy) = {
        let s = state.lock().await;
        (s.risk.cfg.clone(), s.strategy_for(code))
    };

    // Re-buy price guard — don't buy back at/above the stop-out price.
    {
        let mut s = state.lock().await;
        if let Some(guard) = s.last_exit_price.get(code).copied() {
            if price >= guard * (1.0 - cfg.reentry_gap_pct) {
                return Eval::new("재매수가드", format!("직전 손절가 {:.0} 위 → 재매수 보류", guard));
            }
            s.last_exit_price.remove(code);
        }
    }

    // Confirmation bar: find the pattern on the prior bar, require the last closed bar to confirm.
    let (scan_input, confirm): (&[Candle], Option<Candle>) = if cfg.require_confirmation {
        (&closed[..closed.len() - 1], closed.last().cloned())
    } else {
        (closed, None)
    };
    if scan_input.len() < 13 {
        return Eval::new("데이터부족", "패턴 분석 데이터 부족");
    }

    // Candlestick patterns (windowed) + context setups (VWAP/ORB/EMA over the
    // full session) form a single candidate pool, scored the same way.
    let mut candidates = deps.detector.scan(scan_input, tf, 0.0, true, &strategy, 1);
    {
        let ctx = SessionContext::for_tf(scan_input, tf);
        let ema9 = ema_values(scan_input, 9);
        let ema20 = ema_values(scan_input, 20);
        // Only generate bearish setups when the broker can actually execute a
        // short — otherwise they'd just occupy the candidate pool and possibly
        // outscore a real, executable bullish signal for nothing.
        let mut setups = detect_setups(
            scan_input,
            scan_input.len() - 1,
            &ctx,
            &ema9,
            &ema20,
            &strategy.enabled_patterns,
            strategy.allows_short() && deps.broker.supports_short(),
        );
        for s in &mut setups {
            apply_strategy(s, &strategy, false, false, false);
        }
        candidates.append(&mut setups);
    }
    // Keep enabled candidates whose direction the strategy *and* the broker permit
    // (paper mirrors live: neither can execute a new short — see `Broker::supports_short`).
    candidates.retain(|r| {
        strategy.enabled_patterns.contains(&r.pattern_name)
            && ((r.pattern_type == "bullish" && strategy.allows_long())
                || (r.pattern_type == "bearish" && strategy.allows_short() && deps.broker.supports_short()))
    });
    candidates.sort_by(|a, b| b.composite_score.partial_cmp(&a.composite_score).unwrap_or(std::cmp::Ordering::Equal));
    let Some(mut top) = candidates.into_iter().next() else {
        return Eval::new("신호없음", "매매 패턴 미감지");
    };
    let side = Side::from_pattern_type(&top.pattern_type);
    // Defensive fallback — candidates are already filtered above, so this
    // should be unreachable, but keep it in case that filter is ever bypassed.
    if side == Side::Short && !deps.broker.supports_short() {
        return Eval::with_signal("숏차단", format!("{} 감지 · 공매도 미지원(모의·실전 공통)", top.pattern_name), top.composite_score, &top.pattern_name);
    }

    // Confirmation bar — direction-aware (longs break up, shorts break down).
    if let Some(confirm) = &confirm {
        let pattern_high = top.candles_used.iter().map(|c| c.high).fold(f64::MIN, f64::max);
        let pattern_low = top.candles_used.iter().map(|c| c.low).fold(f64::MAX, f64::min);
        let pattern_close = top.candles_used.last().map(|c| c.close).unwrap_or(0.0);
        let confirmed = match side {
            Side::Long => confirm.close > pattern_high || (confirm.is_bull() && confirm.close > pattern_close),
            Side::Short => confirm.close < pattern_low || (confirm.is_bear() && confirm.close < pattern_close),
        };
        if !confirmed {
            tracing::info!("entry unconfirmed {code} ({})", top.pattern_name);
            return Eval::with_signal("확인봉대기", format!("{} 감지 · 확인봉 미충족", top.pattern_name), top.composite_score, &top.pattern_name);
        }
    }

    // Higher-TF trend filter — block trades fighting the upper timeframe.
    if cfg.require_higher_tf_uptrend {
        let mtf = MtfEngine::new(&deps.fetcher, &deps.detector);
        let up = mtf.higher_tf_uptrend(token, code, tf).await;
        let blocked = match side {
            Side::Long => !up,
            Side::Short => up,
        };
        if blocked {
            tracing::info!("entry blocked {code}: higher_tf against {}", side.as_str());
            return Eval::with_signal("상위TF역행", format!("{} 감지 · 상위 TF 역행으로 진입 차단", top.pattern_name), top.composite_score, &top.pattern_name);
        }
    }

    // MTF confluence score → recompute composite.
    {
        let mtf = MtfEngine::new(&deps.fetcher, &deps.detector);
        top.mtf_score = mtf.score(token, code, tf, &top.pattern_type).await;
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
        let (c, w) = s.risk.can_enter(equity);
        (c, w.to_string())
    };
    if !can {
        tracing::info!("entry blocked {code}: {why}");
        return Eval::with_signal("진입제한", format!("{} 감지 · 리스크 한도로 진입 제한 ({why})", top.pattern_name), top.composite_score, &top.pattern_name);
    }

    // Sizing: fixed qty or risk-based, capped by the portfolio-wide max buy
    // amount (total entered across all positions, not just this order) and cash.
    let (mut qty, order_type, max_buy, cash, entered) = {
        let s = state.lock().await;
        let q = s.order.fixed_qty.unwrap_or_else(|| s.risk.position_size(equity, price, atr));
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
    let fill = match side {
        Side::Long => deps.broker.buy(token, code, qty, price, &order_type).await,
        Side::Short => deps.broker.sell_short(token, code, qty, price, &order_type).await,
    };
    if !fill.ok {
        return Eval::with_signal("주문실패", format!("{} 진입 주문 실패", top.pattern_name), top.composite_score, &top.pattern_name);
    }
    let mut s = state.lock().await;
    if side == Side::Long {
        s.cash -= qty as f64 * fill.fill_price; // short opens credit/use no cash in the paper sim
    }
    let (stop, target) = s.risk.stop_and_target(fill.fill_price, atr, side);
    s.risk.register(code, side, fill.fill_price, qty, stop, target);
    s.opened_meta.insert(code.to_string(), (now_iso(), top.pattern_name.clone()));
    s.bars_held.insert(code.to_string(), 0);
    s.last_exit_price.remove(code);
    let action_label = if side == Side::Short { "매도(숏)" } else { "매수" };
    s.append_event(json!({
        "code": code, "name": name_for(code), "side": side.as_str(),
        "type": if side == Side::Short { "sell" } else { "buy" }, "action": "open",
        "price": fill.fill_price,
        "qty": qty, "pnl": 0.0, "pnl_pct": 0.0, "reason": top.pattern_name,
        "ts": candle_ts, "time_label": hhmm_label(),
    }));
    tracing::info!("ENTER {} {code} x{qty} @{:.0} stop={:.0} target={:.0} ({} {:.2})",
        side.as_str(), fill.fill_price, stop, target, top.pattern_name, top.composite_score);
    Eval::with_signal("진입", format!("{} {action_label} {qty}주 @{:.0}", top.pattern_name, fill.fill_price), top.composite_score, &top.pattern_name)
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
    // Direction-aware P&L: a short profits when the exit is *below* the entry.
    let dir = if pos.side == Side::Short { -1.0 } else { 1.0 };
    let ret_pct = if pos.entry != 0.0 { (exit_price - pos.entry) / pos.entry * 100.0 * dir } else { 0.0 };
    deps.journal.record(&TradeRecord {
        mode: deps.broker.mode.as_str().to_string(),
        code: code.to_string(),
        side: pos.side.as_str().to_string(),
        qty,
        entry: pos.entry,
        exit: exit_price,
        pnl: (exit_price - pos.entry) * qty as f64 * dir,
        return_pct: ret_pct,
        reason: reason.to_string(),
        pattern,
        opened_at,
        closed_at: now_iso(),
    });
}
