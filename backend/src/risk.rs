//! Risk management gate (spec section 10-3).
//!
//! Every entry must pass `can_enter()` + `position_size()`. Stops/targets are
//! ATR-based with a trailing stop, and a daily loss limit halts new entries.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fibonacci 1,1,2,3,5,8,… (n≥1) — used for averaging-down quantity multiples.
pub fn fib(n: i64) -> i64 {
    let (mut a, mut b) = (1i64, 1i64);
    for _ in 0..(n - 1).max(0) {
        let next = a + b;
        a = b;
        b = next;
    }
    a
}

/// Tunable risk rules (sizing, stops, cooldowns, averaging, entry quality).
#[derive(Clone)]
pub struct RiskConfig {
    pub max_position_pct: f64,
    pub max_positions: usize,
    pub risk_per_trade_pct: f64,
    pub stop_loss_atr_mult: f64,
    pub take_profit_atr_mult: f64,
    // Manual fixed-percentage stop/take-profit (0 = disabled → use the ATR multiples above).
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub daily_loss_limit_pct: f64,
    pub trailing_stop_atr: f64,
    // Auto-exit condition switches — let the user pick which automatic sell
    // triggers are active. `use_stop_loss` off removes the price stop entirely,
    // both its closed-bar and intrabar (hard-stop) variants (⚠️ only EOD
    // flatten and the daily-loss limit then remain as protection).
    pub use_stop_loss: bool,
    pub use_take_profit: bool,
    pub use_trailing_stop: bool,
    // Re-entry guards (avoid whipsaw: sell low → instantly buy high).
    pub reentry_cooldown_bars: i64,
    pub loss_cooldown_bars: i64,
    pub reentry_gap_pct: f64,
    /// Bars until the stop-out price guard expires (0 = never expires).
    /// Without expiry a stop-out would block re-entry forever once the price
    /// recovers above the exit — killing every valid trend re-entry.
    pub reentry_guard_expire_bars: i64,
    // Fibonacci averaging-down (물타기): add instead of selling on a stop signal.
    pub fib_averaging_enabled: bool,
    pub fib_max_levels: i64,
    // Entry-quality gates (avoid catching a falling knife).
    pub require_confirmation: bool,
    /// How many closed bars after the pattern a confirmation may arrive in.
    /// 1 = the legacy "very next bar only"; larger values keep a detected
    /// pattern alive so a slightly late breakout still triggers the entry.
    pub confirm_window_bars: i64,
    pub require_higher_tf_uptrend: bool,
    /// Relaxation for the higher-TF trend gate: block longs only when the
    /// higher-TF slope is *below* −tolerance (fraction of price per bar).
    /// 0 = strict legacy behavior (any negative slope blocks).
    pub higher_tf_slope_tolerance: f64,
    // Exit stabilization.
    pub min_hold_bars: i64,
    pub hard_stop_intrabar: bool,
    pub hard_stop_buffer_pct: f64,
    // End-of-day flatten (day-trading): block new entries near the close and
    // force-close everything before the session ends (no overnight gap risk).
    pub eod_flatten: bool,
}

impl RiskConfig {
    /// (stop distance, target distance) from an entry price. Manual fixed-%
    /// (>0) takes priority over the ATR multiples. Shared by stop placement,
    /// position sizing, the entry gates, and the backtester so they all judge
    /// the same exits.
    pub fn stop_target_dists(&self, entry: f64, atr: f64) -> (f64, f64) {
        let stop = if self.stop_loss_pct > 0.0 {
            entry * self.stop_loss_pct
        } else {
            atr * self.stop_loss_atr_mult
        };
        let target = if self.take_profit_pct > 0.0 {
            entry * self.take_profit_pct
        } else {
            atr * self.take_profit_atr_mult
        };
        (stop, target)
    }

    /// Serialize the full risk configuration for the status API.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "max_position_pct": self.max_position_pct,
            "max_positions": self.max_positions,
            "risk_per_trade_pct": self.risk_per_trade_pct,
            "stop_loss_atr_mult": self.stop_loss_atr_mult,
            "take_profit_atr_mult": self.take_profit_atr_mult,
            "stop_loss_pct": self.stop_loss_pct,
            "take_profit_pct": self.take_profit_pct,
            "daily_loss_limit_pct": self.daily_loss_limit_pct,
            "trailing_stop_atr": self.trailing_stop_atr,
            "use_stop_loss": self.use_stop_loss,
            "use_take_profit": self.use_take_profit,
            "use_trailing_stop": self.use_trailing_stop,
            "reentry_cooldown_bars": self.reentry_cooldown_bars,
            "loss_cooldown_bars": self.loss_cooldown_bars,
            "reentry_gap_pct": self.reentry_gap_pct,
            "reentry_guard_expire_bars": self.reentry_guard_expire_bars,
            "fib_averaging_enabled": self.fib_averaging_enabled,
            "fib_max_levels": self.fib_max_levels,
            "require_confirmation": self.require_confirmation,
            "confirm_window_bars": self.confirm_window_bars,
            "require_higher_tf_uptrend": self.require_higher_tf_uptrend,
            "higher_tf_slope_tolerance": self.higher_tf_slope_tolerance,
            "min_hold_bars": self.min_hold_bars,
            "hard_stop_intrabar": self.hard_stop_intrabar,
            "hard_stop_buffer_pct": self.hard_stop_buffer_pct,
            "eod_flatten": self.eod_flatten,
        })
    }
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_position_pct: 0.10,
            max_positions: 5,
            risk_per_trade_pct: 0.01,
            stop_loss_atr_mult: 1.5,
            take_profit_atr_mult: 3.0,
            stop_loss_pct: 0.0,
            take_profit_pct: 0.0,
            daily_loss_limit_pct: 0.03,
            trailing_stop_atr: 2.0,
            use_stop_loss: true,
            use_take_profit: true,
            use_trailing_stop: true,
            reentry_cooldown_bars: 1,
            loss_cooldown_bars: 3,
            reentry_gap_pct: 0.0,
            reentry_guard_expire_bars: 20,
            fib_averaging_enabled: false,
            fib_max_levels: 0,
            require_confirmation: true,
            confirm_window_bars: 3,
            require_higher_tf_uptrend: true,
            higher_tf_slope_tolerance: 0.0,
            min_hold_bars: 1,
            hard_stop_intrabar: false,
            hard_stop_buffer_pct: 0.0,
            eod_flatten: true,
        }
    }
}

/// Position direction. Shorts are paper-simulation only (KR retail intraday
/// shorting is effectively unavailable); the live broker rejects short orders.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Long,
    Short,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Long => "long",
            Side::Short => "short",
        }
    }
    /// Map a pattern type (`"bullish"` / `"bearish"`) to a trade side.
    pub fn from_pattern_type(t: &str) -> Self {
        if t == "bearish" { Side::Short } else { Side::Long }
    }
}

/// An open position tracked by the risk manager.
#[derive(Clone, Serialize, Deserialize)]
pub struct Position {
    pub side: Side,
    pub entry: f64,
    pub qty: i64,
    pub stop: f64,
    pub target: f64,
    pub peak: f64, // best excursion: highest price for longs, lowest for shorts
    pub base_qty: i64, // first-entry size; basis for fib averaging multiples
    pub fib_level: i64,
}

/// Holds open positions + the running daily P&L; enforces the risk rules.
pub struct RiskManager {
    pub cfg: RiskConfig,
    daily_pnl: f64,
    /// Equity at the start of the trading day — the base the daily-loss limit
    /// draws down from (so unrealized losses count, not just realized P&L).
    day_start_equity: f64,
    positions: HashMap<String, Position>,
}

impl RiskManager {
    pub fn new(cfg: RiskConfig) -> Self {
        Self { cfg, daily_pnl: 0.0, day_start_equity: 0.0, positions: HashMap::new() }
    }

    pub fn open_positions(&self) -> &HashMap<String, Position> {
        &self.positions
    }
    pub fn position(&self, code: &str) -> Option<&Position> {
        self.positions.get(code)
    }
    pub fn daily_pnl(&self) -> f64 {
        self.daily_pnl
    }

    /// Total notional committed across all open positions (Σ entry×qty, both
    /// longs and shorts) — what the order-level `max_buy_amount` cap is
    /// measured against (a portfolio-wide total, not a per-order limit).
    pub fn total_entered_amount(&self) -> f64 {
        self.positions.values().map(|p| p.entry * p.qty as f64).sum()
    }

    /// Daily-loss check counting *unrealized* losses too: realized P&L against
    /// the limit, plus the equity drawdown from the day-start equity. Shared by
    /// new entries and averaging-down adds (so 물타기 can't bypass the limit).
    pub fn daily_loss_ok(&self, equity: f64) -> bool {
        if self.daily_pnl <= -equity * self.cfg.daily_loss_limit_pct {
            return false;
        }
        if self.day_start_equity > 0.0
            && equity <= self.day_start_equity * (1.0 - self.cfg.daily_loss_limit_pct)
        {
            return false;
        }
        true
    }

    /// Gate new entries on the daily-loss limit and max open positions.
    pub fn can_enter(&self, equity: f64) -> (bool, &'static str) {
        if !self.daily_loss_ok(equity) {
            return (false, "daily_loss_limit_reached");
        }
        if self.positions.len() >= self.cfg.max_positions {
            return (false, "max_positions_reached");
        }
        (true, "ok")
    }

    /// Position size = min(risk-based qty, position-cap qty).
    /// Risk-based sizing uses the *actual* stop distance (manual fixed-% when set).
    pub fn position_size(&self, equity: f64, entry: f64, atr: f64) -> i64 {
        let (stop_distance, _) = self.cfg.stop_target_dists(entry, atr);
        if stop_distance <= 0.0 || entry <= 0.0 {
            return 0;
        }
        let qty_by_risk = (equity * self.cfg.risk_per_trade_pct / stop_distance) as i64;
        let qty_by_cap = (equity * self.cfg.max_position_pct / entry) as i64;
        qty_by_risk.min(qty_by_cap).max(0)
    }

    /// (stop, target) around an entry price for the given side.
    /// Manual fixed-% takes priority when set (>0); otherwise falls back to ATR multiples.
    /// Shorts mirror longs: stop above entry, target below.
    pub fn stop_and_target(&self, entry: f64, atr: f64, side: Side) -> (f64, f64) {
        let (stop_dist, target_dist) = self.cfg.stop_target_dists(entry, atr);
        match side {
            Side::Long => (entry - stop_dist, entry + target_dist),
            Side::Short => (entry + stop_dist, entry - target_dist),
        }
    }

    pub fn register(&mut self, code: &str, side: Side, entry: f64, qty: i64, stop: f64, target: f64) {
        self.positions.insert(
            code.to_string(),
            Position { side, entry, qty, stop, target, peak: entry, base_qty: qty, fib_level: 0 },
        );
    }

    /// Decide whether to exit: returns stop_loss / take_profit / trailing_stop / None.
    /// Comparisons are direction-aware (shorts mirror longs).
    pub fn check_exit(&mut self, code: &str, price: f64, atr: f64) -> Option<&'static str> {
        let (use_sl, use_tp, use_trail) =
            (self.cfg.use_stop_loss, self.cfg.use_take_profit, self.cfg.use_trailing_stop);
        let pos = self.positions.get_mut(code)?;
        match pos.side {
            Side::Long => {
                pos.peak = pos.peak.max(price);
                let trail = pos.peak - atr * self.cfg.trailing_stop_atr;
                if use_sl && price <= pos.stop {
                    Some("stop_loss")
                } else if use_tp && price >= pos.target {
                    Some("take_profit")
                } else if use_trail && price <= trail && price > pos.entry {
                    Some("trailing_stop")
                } else {
                    None
                }
            }
            Side::Short => {
                pos.peak = pos.peak.min(price);
                let trail = pos.peak + atr * self.cfg.trailing_stop_atr;
                if use_sl && price >= pos.stop {
                    Some("stop_loss")
                } else if use_tp && price <= pos.target {
                    Some("take_profit")
                } else if use_trail && price >= trail && price < pos.entry {
                    Some("trailing_stop")
                } else {
                    None
                }
            }
        }
    }

    /// Intrabar protection: live price beyond stop × (1 ∓ buffer)?
    pub fn hard_stop_hit(&self, code: &str, price: f64, buffer_pct: f64) -> bool {
        self.positions.get(code).is_some_and(|p| match p.side {
            Side::Long => price <= p.stop * (1.0 - buffer_pct),
            Side::Short => price >= p.stop * (1.0 + buffer_pct),
        })
    }

    /// Whether another averaging-down level is available (feature on + level free).
    /// Averaging-down is a long-only concept, so shorts never qualify.
    pub fn can_average(&self, code: &str) -> bool {
        if !self.cfg.fib_averaging_enabled || self.cfg.fib_max_levels <= 0 {
            return false;
        }
        self.positions
            .get(code)
            .is_some_and(|p| p.side == Side::Long && p.fib_level < self.cfg.fib_max_levels)
    }

    /// Quantity to buy at the next averaging level = base_qty × fib(level+1).
    pub fn next_fib_qty(&self, code: &str) -> i64 {
        self.positions
            .get(code)
            .map_or(0, |p| p.base_qty * fib(p.fib_level + 1))
    }

    /// Add to a position, lower the average entry, and reset stop/target.
    pub fn average_down(&mut self, code: &str, add_price: f64, add_qty: i64, atr: f64) -> Option<Position> {
        let (stop, target);
        {
            let pos = self.positions.get(code)?;
            if add_qty <= 0 {
                return None;
            }
            let side = pos.side;
            let new_qty = pos.qty + add_qty;
            let new_entry = (pos.entry * pos.qty as f64 + add_price * add_qty as f64) / new_qty as f64;
            (stop, target) = self.stop_and_target(new_entry, atr, side);
            let pos = self.positions.get_mut(code).unwrap();
            pos.entry = new_entry;
            pos.qty = new_qty;
            pos.fib_level += 1;
            pos.peak = match side {
                Side::Long => add_price.max(new_entry),
                Side::Short => add_price.min(new_entry),
            };
            pos.stop = stop;
            pos.target = target;
        }
        self.positions.get(code).cloned()
    }

    /// Partial close: subtract sold qty; remove the position when it hits zero.
    /// Returns true if the position is now fully closed.
    pub fn reduce(&mut self, code: &str, qty_sold: i64, pnl: f64) -> bool {
        let Some(pos) = self.positions.get_mut(code) else {
            return true;
        };
        self.daily_pnl += pnl;
        pos.qty -= qty_sold;
        if pos.qty <= 0 {
            self.positions.remove(code);
            true
        } else {
            false
        }
    }

    /// Reset the daily P&L and re-anchor the drawdown base at today's opening equity.
    pub fn reset_daily(&mut self, day_start_equity: f64) {
        self.daily_pnl = 0.0;
        self.day_start_equity = day_start_equity;
    }

    // --- persistence support (engine snapshot restore) ---

    /// Replace all open positions (snapshot restore after a backend restart).
    pub fn restore_positions(&mut self, positions: HashMap<String, Position>) {
        self.positions = positions;
    }

    /// Restore the running daily P&L (only for a same-day snapshot).
    pub fn set_daily_pnl(&mut self, pnl: f64) {
        self.daily_pnl = pnl;
    }

    pub fn day_start_equity(&self) -> f64 {
        self.day_start_equity
    }
    pub fn set_day_start_equity(&mut self, v: f64) {
        self.day_start_equity = v;
    }

    /// Overwrite one position's entry/qty from the real account (live reconcile).
    /// Stop/target are kept — they were placed off the strategy, not the fill.
    pub fn sync_position(&mut self, code: &str, entry: f64, qty: i64) {
        if let Some(p) = self.positions.get_mut(code) {
            p.entry = entry;
            p.qty = qty;
        }
    }

    /// Drop a position without journaling (it no longer exists in the account).
    pub fn forget(&mut self, code: &str) -> Option<Position> {
        self.positions.remove(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> RiskManager {
        RiskManager::new(RiskConfig {
            stop_loss_atr_mult: 1.0,
            take_profit_atr_mult: 2.0,
            trailing_stop_atr: 1.0,
            ..Default::default()
        })
    }

    #[test]
    fn short_stop_target_mirror_long() {
        let m = mgr();
        let (ls, lt) = m.stop_and_target(100.0, 5.0, Side::Long);
        let (ss, st) = m.stop_and_target(100.0, 5.0, Side::Short);
        assert_eq!((ls, lt), (95.0, 110.0));
        assert_eq!((ss, st), (105.0, 90.0)); // stop above, target below
    }

    #[test]
    fn short_exit_hits_stop_above_entry() {
        let mut m = mgr();
        m.register("X", Side::Short, 100.0, 10, 105.0, 90.0);
        assert_eq!(m.check_exit("X", 101.0, 5.0), None);
        assert_eq!(m.check_exit("X", 106.0, 5.0), Some("stop_loss")); // price rose into stop
    }

    #[test]
    fn short_exit_hits_target_below_entry() {
        let mut m = mgr();
        m.register("X", Side::Short, 100.0, 10, 105.0, 90.0);
        assert_eq!(m.check_exit("X", 89.0, 5.0), Some("take_profit"));
    }

    #[test]
    fn manual_pct_overrides_atr_for_stops_and_sizing() {
        let m = RiskManager::new(RiskConfig {
            stop_loss_pct: 0.03,
            take_profit_pct: 0.06,
            ..Default::default()
        });
        // Manual 3%/6% beats the ATR multiples.
        let (stop, target) = m.stop_and_target(100.0, 5.0, Side::Long);
        assert_eq!((stop, target), (97.0, 106.0));
        // Sizing uses the same manual stop distance: risk 1% of 1M = 10,000원,
        // stop 3원 → 3333주, capped by 10% position cap → 1000주.
        assert_eq!(m.position_size(1_000_000.0, 100.0, 5.0), 1000);
    }

    #[test]
    fn total_entered_amount_sums_both_sides() {
        let mut m = mgr();
        assert_eq!(m.total_entered_amount(), 0.0);
        m.register("L", Side::Long, 100.0, 10, 95.0, 110.0); // 1,000
        m.register("S", Side::Short, 200.0, 5, 210.0, 180.0); // 1,000
        assert_eq!(m.total_entered_amount(), 2000.0);
    }

    #[test]
    fn short_excluded_from_averaging() {
        let mut m = RiskManager::new(RiskConfig { fib_averaging_enabled: true, fib_max_levels: 3, ..Default::default() });
        m.register("L", Side::Long, 100.0, 10, 95.0, 110.0);
        m.register("S", Side::Short, 100.0, 10, 105.0, 90.0);
        assert!(m.can_average("L"));
        assert!(!m.can_average("S"));
    }
}
