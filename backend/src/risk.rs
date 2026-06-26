//! Risk management gate (spec section 10-3).
//!
//! Every entry must pass `can_enter()` + `position_size()`. Stops/targets are
//! ATR-based with a trailing stop, and a daily loss limit halts new entries.

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
    // Re-entry guards (avoid whipsaw: sell low → instantly buy high).
    pub reentry_cooldown_bars: i64,
    pub loss_cooldown_bars: i64,
    pub reentry_gap_pct: f64,
    // Fibonacci averaging-down (물타기): add instead of selling on a stop signal.
    pub fib_averaging_enabled: bool,
    pub fib_max_levels: i64,
    // Entry-quality gates (avoid catching a falling knife).
    pub require_confirmation: bool,
    pub require_higher_tf_uptrend: bool,
    // Exit stabilization.
    pub min_hold_bars: i64,
    pub hard_stop_intrabar: bool,
    pub hard_stop_buffer_pct: f64,
}

impl RiskConfig {
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
            "reentry_cooldown_bars": self.reentry_cooldown_bars,
            "loss_cooldown_bars": self.loss_cooldown_bars,
            "reentry_gap_pct": self.reentry_gap_pct,
            "fib_averaging_enabled": self.fib_averaging_enabled,
            "fib_max_levels": self.fib_max_levels,
            "require_confirmation": self.require_confirmation,
            "require_higher_tf_uptrend": self.require_higher_tf_uptrend,
            "min_hold_bars": self.min_hold_bars,
            "hard_stop_intrabar": self.hard_stop_intrabar,
            "hard_stop_buffer_pct": self.hard_stop_buffer_pct,
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
            reentry_cooldown_bars: 1,
            loss_cooldown_bars: 3,
            reentry_gap_pct: 0.0,
            fib_averaging_enabled: false,
            fib_max_levels: 0,
            require_confirmation: true,
            require_higher_tf_uptrend: true,
            min_hold_bars: 1,
            hard_stop_intrabar: false,
            hard_stop_buffer_pct: 0.0,
        }
    }
}

/// An open position tracked by the risk manager.
#[derive(Clone)]
pub struct Position {
    pub entry: f64,
    pub qty: i64,
    pub stop: f64,
    pub target: f64,
    pub peak: f64,
    pub base_qty: i64, // first-entry size; basis for fib averaging multiples
    pub fib_level: i64,
}

/// Holds open positions + the running daily P&L; enforces the risk rules.
pub struct RiskManager {
    pub cfg: RiskConfig,
    daily_pnl: f64,
    positions: HashMap<String, Position>,
}

impl RiskManager {
    pub fn new(cfg: RiskConfig) -> Self {
        Self { cfg, daily_pnl: 0.0, positions: HashMap::new() }
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

    /// Gate new entries on the daily-loss limit and max open positions.
    pub fn can_enter(&self, equity: f64) -> (bool, &'static str) {
        if self.daily_pnl <= -equity * self.cfg.daily_loss_limit_pct {
            return (false, "daily_loss_limit_reached");
        }
        if self.positions.len() >= self.cfg.max_positions {
            return (false, "max_positions_reached");
        }
        (true, "ok")
    }

    /// Position size = min(risk-based qty, position-cap qty).
    pub fn position_size(&self, equity: f64, entry: f64, atr: f64) -> i64 {
        let stop_distance = atr * self.cfg.stop_loss_atr_mult;
        if stop_distance <= 0.0 || entry <= 0.0 {
            return 0;
        }
        let qty_by_risk = (equity * self.cfg.risk_per_trade_pct / stop_distance) as i64;
        let qty_by_cap = (equity * self.cfg.max_position_pct / entry) as i64;
        qty_by_risk.min(qty_by_cap).max(0)
    }

    /// (stop, target) around an entry price.
    /// Manual fixed-% takes priority when set (>0); otherwise falls back to ATR multiples.
    pub fn stop_and_target(&self, entry: f64, atr: f64) -> (f64, f64) {
        let stop = if self.cfg.stop_loss_pct > 0.0 {
            entry * (1.0 - self.cfg.stop_loss_pct)
        } else {
            entry - atr * self.cfg.stop_loss_atr_mult
        };
        let target = if self.cfg.take_profit_pct > 0.0 {
            entry * (1.0 + self.cfg.take_profit_pct)
        } else {
            entry + atr * self.cfg.take_profit_atr_mult
        };
        (stop, target)
    }

    pub fn register(&mut self, code: &str, entry: f64, qty: i64, stop: f64, target: f64) {
        self.positions.insert(
            code.to_string(),
            Position { entry, qty, stop, target, peak: entry, base_qty: qty, fib_level: 0 },
        );
    }

    /// Decide whether to exit: returns stop_loss / take_profit / trailing_stop / None.
    pub fn check_exit(&mut self, code: &str, price: f64, atr: f64) -> Option<&'static str> {
        let pos = self.positions.get_mut(code)?;
        pos.peak = pos.peak.max(price);
        let trail = pos.peak - atr * self.cfg.trailing_stop_atr;
        if price <= pos.stop {
            Some("stop_loss")
        } else if price >= pos.target {
            Some("take_profit")
        } else if price <= trail && price > pos.entry {
            Some("trailing_stop")
        } else {
            None
        }
    }

    /// Intrabar protection: live price at/below stop × (1 − buffer)?
    pub fn hard_stop_hit(&self, code: &str, price: f64, buffer_pct: f64) -> bool {
        self.positions
            .get(code)
            .is_some_and(|p| price <= p.stop * (1.0 - buffer_pct))
    }

    /// Whether another averaging-down level is available (feature on + level free).
    pub fn can_average(&self, code: &str) -> bool {
        if !self.cfg.fib_averaging_enabled || self.cfg.fib_max_levels <= 0 {
            return false;
        }
        self.positions.get(code).is_some_and(|p| p.fib_level < self.cfg.fib_max_levels)
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
            let new_qty = pos.qty + add_qty;
            let new_entry = (pos.entry * pos.qty as f64 + add_price * add_qty as f64) / new_qty as f64;
            (stop, target) = self.stop_and_target(new_entry, atr);
            let pos = self.positions.get_mut(code).unwrap();
            pos.entry = new_entry;
            pos.qty = new_qty;
            pos.fib_level += 1;
            pos.peak = add_price.max(new_entry);
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

    pub fn reset_daily(&mut self) {
        self.daily_pnl = 0.0;
    }
}
