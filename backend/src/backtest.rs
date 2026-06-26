//! Cost-aware, look-ahead-safe backtester + walk-forward (spec sections 6-3, 11-1).

use crate::candle::Candle;
use crate::pattern::{compute_atr, PatternDetector};
use crate::risk::RiskConfig;
use crate::strategy::StrategyConfig;
use crate::timeframe::Timeframe;
use serde_json::{json, Value};

/// Korean equity round-trip cost model (fee + tax + slippage).
#[derive(Clone, Copy)]
pub struct CostModel {
    pub fee_rate: f64,
    pub tax_rate: f64,
    pub slippage_rate: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self { fee_rate: 0.00015, tax_rate: 0.0015, slippage_rate: 0.0008 }
    }
}

impl CostModel {
    /// Round-trip cost fraction: 2× fee + sell tax + 2× slippage.
    pub fn round_trip_cost(&self) -> f64 {
        self.fee_rate * 2.0 + self.tax_rate + self.slippage_rate * 2.0
    }
}

/// Performance stats over a list of per-trade net returns (%).
fn summarize(returns: &[f64], extra: Value) -> Value {
    let mut base = json!({
        "signals": 0, "win_rate": 0.0, "avg_return": 0.0,
        "max_drawdown": 0.0, "sharpe_ratio": 0.0, "profit_factor": 0.0,
    });
    if let Value::Object(m) = &extra {
        for (k, v) in m {
            base[k] = v.clone();
        }
    }
    if returns.is_empty() {
        return base;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let wins: f64 = returns.iter().filter(|&&r| r > 0.0).sum();
    let losses: f64 = returns.iter().filter(|&&r| r <= 0.0).sum();
    let win_count = returns.iter().filter(|&&r| r > 0.0).count();
    // Max drawdown of the cumulative equity curve.
    let (mut equity, mut peak, mut mdd): (f64, f64, f64) = (0.0, f64::MIN, 0.0);
    for &r in returns {
        equity += r;
        peak = peak.max(equity);
        mdd = mdd.min(equity - peak);
    }
    let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    let pf = if losses != 0.0 { wins / losses.abs() } else { f64::INFINITY };
    base["signals"] = json!(returns.len());
    base["win_rate"] = json!(win_count as f64 / n);
    base["avg_return"] = json!(mean);
    base["max_drawdown"] = json!(mdd);
    base["sharpe_ratio"] = json!(mean / (std + 1e-9));
    base["profit_factor"] = json!(pf);
    base
}

/// Pattern-only backtest: enter next bar's open, exit after fixed `hold_bars`.
pub fn run_backtest(
    candles: &[Candle],
    pattern_name: &str,
    tf: Timeframe,
    hold_bars: usize,
    cost: &CostModel,
    side: &str,
    detector: &PatternDetector,
) -> Value {
    let rt_cost = cost.round_trip_cost() * 100.0;
    let default_cfg = StrategyConfig::default();
    let mut returns: Vec<f64> = vec![];
    if candles.len() <= hold_bars + 1 {
        return summarize(&returns, json!({"pattern": pattern_name}));
    }
    for i in 0..candles.len() - hold_bars - 1 {
        let start = i.saturating_sub(40);
        let window = &candles[start..=i];
        if window.len() < 13 {
            continue;
        }
        let found = detector.scan(window, tf, 0.0, false, &default_cfg, 1);
        if !found.iter().any(|p| p.pattern_name == pattern_name) {
            continue;
        }
        let entry = candles[i + 1].open;
        if entry <= 0.0 {
            continue;
        }
        let exit = candles[i + 1 + hold_bars].close;
        let mut gross = (exit - entry) / entry * 100.0;
        if side == "short" {
            gross = -gross;
        }
        returns.push(gross - rt_cost);
    }
    summarize(&returns, json!({"pattern": pattern_name}))
}

/// Aggregate `run_backtest` across many patterns (signal-weighted).
pub fn backtest_many(candles: &[Candle], patterns: &[String], tf: Timeframe, hold_bars: usize, cost: &CostModel) -> Value {
    let detector = PatternDetector;
    let by_pattern: Vec<Value> = patterns
        .iter()
        .map(|p| run_backtest(candles, p, tf, hold_bars, cost, "long", &detector))
        .collect();
    let sig = |b: &Value| b.get("signals").and_then(Value::as_u64).unwrap_or(0) as f64;
    let total: f64 = by_pattern.iter().map(sig).sum();
    let (avg, win) = if total == 0.0 {
        (0.0, 0.0)
    } else {
        let avg = by_pattern.iter().map(|b| b["avg_return"].as_f64().unwrap_or(0.0) * sig(b)).sum::<f64>() / total;
        let win = by_pattern.iter().map(|b| b["win_rate"].as_f64().unwrap_or(0.0) * sig(b)).sum::<f64>() / total;
        (avg, win)
    };
    json!({
        "total_signals": total as u64,
        "avg_return": avg,
        "win_rate": win,
        "by_pattern": by_pattern,
    })
}

/// As-traded backtest: same gates + ATR stop/target/trailing as the live engine.
pub fn run_strategy_backtest(
    candles: &[Candle],
    cfg: &StrategyConfig,
    tf: Timeframe,
    risk: &RiskConfig,
    cost: &CostModel,
    max_hold_bars: usize,
) -> Value {
    let detector = PatternDetector;
    let rt_cost = cost.round_trip_cost() * 100.0;
    let cost_pct = rt_cost;
    let rr = if risk.stop_loss_atr_mult != 0.0 {
        risk.take_profit_atr_mult / risk.stop_loss_atr_mult
    } else {
        0.0
    };
    let n = candles.len();
    let mut returns: Vec<f64> = vec![];
    let mut i = 0;
    while i + 1 < n {
        let start = i.saturating_sub(40);
        let window = &candles[start..=i];
        if window.len() < 13 {
            i += 1;
            continue;
        }
        let found = detector.scan(window, tf, 0.0, true, cfg, 1);
        let has_cand = found.iter().any(|r| {
            cfg.enabled_patterns.contains(&r.pattern_name)
                && r.pattern_type == "bullish"
                && r.composite_score >= cfg.entry_threshold
                && (!cfg.require_volume_confirm || r.volume_confirmed)
        });
        if !has_cand {
            i += 1;
            continue;
        }
        let entry = candles[i + 1].open;
        let atr = compute_atr(window, 14);
        if entry <= 0.0 || atr <= 0.0 {
            i += 1;
            continue;
        }
        // Reward/risk + edge-over-cost gates (same as engine `passes_entry_gates`).
        if cfg.min_reward_risk > 0.0 && rr < cfg.min_reward_risk {
            i += 1;
            continue;
        }
        let target_move_pct = atr * risk.take_profit_atr_mult / entry * 100.0;
        if cfg.min_edge_over_cost > 0.0 && target_move_pct < cost_pct * cfg.min_edge_over_cost {
            i += 1;
            continue;
        }
        let stop = entry - atr * risk.stop_loss_atr_mult;
        let target = entry + atr * risk.take_profit_atr_mult;
        let mut peak = entry;
        let last_idx = (i + 1 + max_hold_bars).min(n - 1);
        let (mut exit_price, mut exit_idx) = (candles[last_idx].close, last_idx);
        for j in (i + 1)..(i + 1 + max_hold_bars).min(n) {
            let c = candles[j].close;
            peak = peak.max(c);
            let trail = peak - atr * risk.trailing_stop_atr;
            if c <= stop {
                exit_price = stop;
                exit_idx = j;
                break;
            }
            if c >= target {
                exit_price = target;
                exit_idx = j;
                break;
            }
            if c <= trail && c > entry {
                exit_price = c;
                exit_idx = j;
                break;
            }
        }
        let gross = (exit_price - entry) / entry * 100.0;
        returns.push(gross - rt_cost);
        i = exit_idx + 1; // resume scanning after the exit (no overlapping positions)
    }
    summarize(&returns, json!({"strategy": cfg.name, "round_trip_cost_pct": rt_cost, "reward_risk": rr}))
}

/// Walk-forward (train, test) splits over the candle series (spec section 11-1).
fn walk_forward_split(candles: &[Candle], n_folds: usize) -> Vec<(&[Candle], &[Candle])> {
    let n = candles.len();
    let fold_size = n / (n_folds + 1);
    if fold_size < 15 {
        return vec![(&candles[..n / 2], &candles[n / 2..])];
    }
    (0..n_folds)
        .map(|k| {
            let train_end = fold_size * (k + 1);
            let test_end = (train_end + fold_size).min(n);
            (&candles[..train_end], &candles[train_end..test_end])
        })
        .collect()
}

/// Walk-forward OOS evaluation under live-equivalent conditions (원인 5).
pub fn evaluate_strategy_live(candles: &[Candle], cfg: &StrategyConfig, tf: Timeframe, risk: &RiskConfig, cost: &CostModel) -> Value {
    let oos: Vec<Value> = walk_forward_split(candles, 4)
        .into_iter()
        .map(|(_train, test)| run_strategy_backtest(test, cfg, tf, risk, cost, 25))
        .filter(|r| r["signals"].as_u64().unwrap_or(0) > 0)
        .collect();
    if oos.is_empty() {
        return json!({"oos_avg_return": 0.0, "oos_consistency": 0.0, "oos_samples": 0,
                      "oos_total_signals": 0, "tradeable": false});
    }
    let avg = oos.iter().map(|r| r["avg_return"].as_f64().unwrap_or(0.0)).sum::<f64>() / oos.len() as f64;
    let consistency = oos.iter().filter(|r| r["avg_return"].as_f64().unwrap_or(0.0) > 0.0).count() as f64 / oos.len() as f64;
    let total_sig: u64 = oos.iter().map(|r| r["signals"].as_u64().unwrap_or(0)).sum();
    json!({
        "oos_avg_return": avg,
        "oos_consistency": consistency,
        "oos_samples": oos.len(),
        "oos_total_signals": total_sig,
        // Recommend live only when OOS net expectancy is positive and consistent.
        "tradeable": avg > 0.0 && consistency >= 0.6 && total_sig >= 10,
    })
}
