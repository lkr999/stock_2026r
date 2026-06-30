//! Caginalp & Laurent (1998) 8-pattern detector + modern overlays (sections 4, 9).
//!
//! All eight reversal patterns are rule-based and deterministic. ATR-adaptive
//! thresholds and volume confirmation (section 9) are applied as overlays; the
//! composite score is computed via `StrategyConfig` (section 10-1), not hardcoded.

use crate::candle::Candle;
use crate::session::SessionContext;
use crate::strategy::{Source, StrategyConfig};
use crate::timeframe::Timeframe;
use serde::Serialize;
use std::collections::HashMap;

/// A detected pattern plus its overlay scores. Serialized straight to the API.
#[derive(Clone, Serialize)]
pub struct PatternResult {
    pub pattern_name: String,
    pub pattern_type: String, // "bullish" | "bearish"
    pub detected_at: String,
    pub confidence: f64,
    pub candles_used: Vec<Candle>,
    pub expected_5: f64,
    pub expected_10: f64,
    pub expected_25: f64,
    pub atr_normalized: bool,
    pub volume_confirmed: bool,
    pub mtf_score: f64,
    pub ml_score: f64,
    pub gaf_score: f64,
    pub composite_score: f64,
}

/// Caginalp & Laurent (1998) Table 3 — 5/10/25-bar excess returns (%).
fn paper_returns(name: &str) -> (f64, f64, f64) {
    match name {
        "three_white_soldiers" => (1.78, 2.43, 3.21),
        "morning_star" => (1.45, 1.89, 2.67),
        "hammer" => (0.89, 1.34, 2.11),
        "bullish_engulfing" => (1.12, 1.56, 2.34),
        "three_black_crows" => (-1.65, -2.28, -3.10),
        "evening_star" => (-1.38, -1.82, -2.55),
        "hanging_man" => (-0.76, -1.21, -1.98),
        "bearish_engulfing" => (-1.05, -1.49, -2.22),
        _ => (0.0, 0.0, 0.0),
    }
}

/// Every single-/few-bar pattern name (used by the batch backtest set).
pub const ALL_PATTERNS: [&str; 16] = [
    "three_white_soldiers",
    "morning_star",
    "hammer",
    "bullish_engulfing",
    "three_black_crows",
    "evening_star",
    "hanging_man",
    "bearish_engulfing",
    "pin_bar_bull",
    "pin_bar_bear",
    "inside_bar_break_up",
    "inside_bar_break_down",
    "tweezer_bottom",
    "tweezer_top",
    "marubozu_bull",
    "marubozu_bear",
];

/// Context-aware day-trading setup names (VWAP / ORB / EMA pullback).
/// These are detected by [`detect_setups`], not the windowed [`PatternDetector`].
pub const SETUP_PATTERNS: [&str; 8] = [
    "vwap_reclaim",
    "vwap_loss",
    "vwap_bounce",
    "vwap_reject",
    "orb_breakout",
    "orb_breakdown",
    "ema_pullback_long",
    "ema_pullback_short",
];

/// Least-squares slope of the closing prices (trend direction).
pub fn trend_slope(candles: &[Candle]) -> f64 {
    let n = candles.len();
    if n < 2 {
        return 0.0;
    }
    let nf = n as f64;
    let xs: Vec<f64> = (0..n).map(|i| i as f64).collect();
    let ys: Vec<f64> = candles.iter().map(|c| c.close).collect();
    let mean_x = (nf - 1.0) / 2.0;
    let mean_y = ys.iter().sum::<f64>() / nf;
    let cov: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mean_x) * (y - mean_y)).sum();
    let var: f64 = xs.iter().map(|x| (x - mean_x).powi(2)).sum();
    if var == 0.0 { 0.0 } else { cov / var }
}

fn avg_body(candles: &[Candle]) -> f64 {
    let m = candles.iter().map(|c| c.body()).sum::<f64>() / candles.len().max(1) as f64;
    if m == 0.0 { 1e-9 } else { m }
}

fn result(name: &str, ptype: &str, ts: &str, conf: f64, used: Vec<Candle>) -> PatternResult {
    let (e5, e10, e25) = paper_returns(name);
    PatternResult {
        pattern_name: name.into(),
        pattern_type: ptype.into(),
        detected_at: ts.into(),
        confidence: conf.clamp(0.0, 1.0),
        candles_used: used,
        expected_5: e5,
        expected_10: e10,
        expected_25: e25,
        atr_normalized: false,
        volume_confirmed: false,
        mtf_score: 0.0,
        ml_score: 0.0,
        gaf_score: 0.0,
        composite_score: 0.0,
    }
}

// --------------------------------------------------------------------------
// eight pattern detectors — each inspects the *last* bar(s) of the window
// --------------------------------------------------------------------------

fn three_white_soldiers(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 3 || trend_slope(&c[n - lookback - 3..n - 3]) >= 0.0 {
        return None;
    }
    let (c1, c2, c3) = (&c[n - 3], &c[n - 2], &c[n - 1]);
    if !(c1.is_bull() && c2.is_bull() && c3.is_bull()) {
        return None;
    }
    if !(c1.open < c2.open && c2.open < c1.open.max(c1.close)) {
        return None;
    }
    if !(c2.open < c3.open && c3.open < c2.open.max(c2.close)) {
        return None;
    }
    if !(c1.close < c2.close && c2.close < c3.close) {
        return None;
    }
    let ratios = [c1.body_ratio(), c2.body_ratio(), c3.body_ratio()];
    let min_ratio = ratios.iter().cloned().fold(f64::INFINITY, f64::min);
    if min_ratio < 0.60 {
        return None;
    }
    let conf = min_ratio / 0.60 * 0.5
        + (c3.close - c1.open) / avg_body(&[c1.clone(), c2.clone(), c3.clone()]) / 3.0 * 0.5;
    Some(result("three_white_soldiers", "bullish", &c3.ts, conf, vec![c1.clone(), c2.clone(), c3.clone()]))
}

fn three_black_crows(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 3 || trend_slope(&c[n - lookback - 3..n - 3]) <= 0.0 {
        return None;
    }
    let (c1, c2, c3) = (&c[n - 3], &c[n - 2], &c[n - 1]);
    if !(c1.is_bear() && c2.is_bear() && c3.is_bear()) {
        return None;
    }
    if !(c1.close > c2.close && c2.close > c3.close) {
        return None;
    }
    if !(c2.open < c1.open && c3.open < c2.open) {
        return None;
    }
    let min_ratio = [c1.body_ratio(), c2.body_ratio(), c3.body_ratio()]
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min);
    if min_ratio < 0.60 {
        return None;
    }
    let conf = min_ratio / 0.60 * 0.6 + 0.4;
    Some(result("three_black_crows", "bearish", &c3.ts, conf, vec![c1.clone(), c2.clone(), c3.clone()]))
}

fn morning_star(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 3 || trend_slope(&c[n - lookback - 3..n - 3]) >= 0.0 {
        return None;
    }
    let (c1, c2, c3) = (&c[n - 3], &c[n - 2], &c[n - 1]);
    if !c1.is_bear() || c1.body_ratio() < 0.70 {
        return None;
    }
    if c2.open.max(c2.close) >= c1.open.min(c1.close) {
        return None;
    }
    if !c3.is_bull() {
        return None;
    }
    let recovery = (c3.close - c2.close) / if c1.body() == 0.0 { 1e-9 } else { c1.body() };
    if recovery < 0.50 {
        return None;
    }
    let conf = recovery * 0.6 + c1.body_ratio() * 0.4;
    Some(result("morning_star", "bullish", &c3.ts, conf, vec![c1.clone(), c2.clone(), c3.clone()]))
}

fn evening_star(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 3 || trend_slope(&c[n - lookback - 3..n - 3]) <= 0.0 {
        return None;
    }
    let (c1, c2, c3) = (&c[n - 3], &c[n - 2], &c[n - 1]);
    if !c1.is_bull() || c1.body_ratio() < 0.70 {
        return None;
    }
    if c2.open.min(c2.close) <= c1.open.max(c1.close) {
        return None;
    }
    if !c3.is_bear() {
        return None;
    }
    let decline = (c2.open - c3.close) / if c1.body() == 0.0 { 1e-9 } else { c1.body() };
    if decline < 0.50 {
        return None;
    }
    let conf = decline * 0.6 + c1.body_ratio() * 0.4;
    Some(result("evening_star", "bearish", &c3.ts, conf, vec![c1.clone(), c2.clone(), c3.clone()]))
}

/// Shared hammer/hanging-man geometry test → (valid, confidence).
fn hammer_shape(c: &Candle) -> (bool, f64) {
    if c.range() < 1e-6 || c.lower_shadow() < c.body() * 2.0 || c.upper_shadow() > c.body() * 0.3 {
        return (false, 0.0);
    }
    let body_pos = (c.open.min(c.close) - c.low) / c.range();
    if body_pos < 0.60 {
        return (false, 0.0);
    }
    let denom = if c.body() * 2.0 == 0.0 { 1e-9 } else { c.body() * 2.0 };
    let conf = (c.lower_shadow() / denom) * 0.7 + body_pos * 0.3;
    (true, conf.min(1.0))
}

fn hammer(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 1 || trend_slope(&c[n - lookback - 1..n - 1]) >= 0.0 {
        return None;
    }
    let last = &c[n - 1];
    let (valid, conf) = hammer_shape(last);
    valid.then(|| result("hammer", "bullish", &last.ts, conf, vec![last.clone()]))
}

fn hanging_man(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 1 || trend_slope(&c[n - lookback - 1..n - 1]) <= 0.0 {
        return None;
    }
    let last = &c[n - 1];
    let (valid, conf) = hammer_shape(last);
    valid.then(|| result("hanging_man", "bearish", &last.ts, conf, vec![last.clone()]))
}

fn bullish_engulfing(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 2 || trend_slope(&c[n - lookback - 2..n - 2]) >= 0.0 {
        return None;
    }
    let (c1, c2) = (&c[n - 2], &c[n - 1]);
    if !c1.is_bear() || !c2.is_bull() || !(c2.open < c1.close && c2.close > c1.open) {
        return None;
    }
    let engulf = c2.body() / if c1.body() == 0.0 { 1e-9 } else { c1.body() };
    let conf = (engulf - 1.0) * 0.5 + 0.5;
    Some(result("bullish_engulfing", "bullish", &c2.ts, conf, vec![c1.clone(), c2.clone()]))
}

fn bearish_engulfing(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 2 || trend_slope(&c[n - lookback - 2..n - 2]) <= 0.0 {
        return None;
    }
    let (c1, c2) = (&c[n - 2], &c[n - 1]);
    if !c1.is_bull() || !c2.is_bear() || !(c2.open > c1.close && c2.close < c1.open) {
        return None;
    }
    let engulf = c2.body() / if c1.body() == 0.0 { 1e-9 } else { c1.body() };
    let conf = (engulf - 1.0) * 0.5 + 0.5;
    Some(result("bearish_engulfing", "bearish", &c2.ts, conf, vec![c1.clone(), c2.clone()]))
}

// --------------------------------------------------------------------------
// intraday candlestick patterns (단타 보강) — pin bar / inside-bar break /
// tweezer / marubozu. Single-/few-bar geometry, so they slot into the same
// windowed detector interface as the eight reversal patterns above.
// --------------------------------------------------------------------------

/// Bullish rejection (pin) bar: long lower wick, close in the upper third.
fn pin_bar_bull(c: &[Candle], _lookback: usize) -> Option<PatternResult> {
    let last = c.last()?;
    let range = last.range();
    if last.lower_shadow() < range * 0.6 || last.upper_shadow() > range * 0.2 {
        return None;
    }
    if (last.close - last.low) / range < 0.6 {
        return None;
    }
    let conf = (last.lower_shadow() / range).min(1.0) * 0.7 + 0.3;
    Some(result("pin_bar_bull", "bullish", &last.ts, conf, vec![last.clone()]))
}

/// Bearish rejection (pin) bar: long upper wick, close in the lower third.
fn pin_bar_bear(c: &[Candle], _lookback: usize) -> Option<PatternResult> {
    let last = c.last()?;
    let range = last.range();
    if last.upper_shadow() < range * 0.6 || last.lower_shadow() > range * 0.2 {
        return None;
    }
    if (last.high - last.close) / range < 0.6 {
        return None;
    }
    let conf = (last.upper_shadow() / range).min(1.0) * 0.7 + 0.3;
    Some(result("pin_bar_bear", "bearish", &last.ts, conf, vec![last.clone()]))
}

/// Inside-bar bullish breakout: inside bar contracts, next bar closes above the mother high.
fn inside_bar_break_up(c: &[Candle], _lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < 3 {
        return None;
    }
    let (mother, inside, brk) = (&c[n - 3], &c[n - 2], &c[n - 1]);
    if inside.high > mother.high || inside.low < mother.low {
        return None;
    }
    if !brk.is_bull() || brk.close <= mother.high {
        return None;
    }
    let conf = (0.55 + (brk.close - mother.high) / mother.range() * 0.45).min(1.0);
    Some(result("inside_bar_break_up", "bullish", &brk.ts, conf, vec![mother.clone(), inside.clone(), brk.clone()]))
}

/// Inside-bar bearish breakout: inside bar contracts, next bar closes below the mother low.
fn inside_bar_break_down(c: &[Candle], _lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < 3 {
        return None;
    }
    let (mother, inside, brk) = (&c[n - 3], &c[n - 2], &c[n - 1]);
    if inside.high > mother.high || inside.low < mother.low {
        return None;
    }
    if !brk.is_bear() || brk.close >= mother.low {
        return None;
    }
    let conf = (0.55 + (mother.low - brk.close) / mother.range() * 0.45).min(1.0);
    Some(result("inside_bar_break_down", "bearish", &brk.ts, conf, vec![mother.clone(), inside.clone(), brk.clone()]))
}

/// Tweezer bottom: matching lows after a down move, bear→bull pair.
fn tweezer_bottom(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 2 || trend_slope(&c[n - lookback - 2..n - 2]) >= 0.0 {
        return None;
    }
    let (c1, c2) = (&c[n - 2], &c[n - 1]);
    let tol = (c1.range() + c2.range()) / 2.0 * 0.1;
    if (c1.low - c2.low).abs() > tol || !c1.is_bear() || !c2.is_bull() {
        return None;
    }
    let conf = (0.6 + c2.body_ratio() * 0.4).min(1.0);
    Some(result("tweezer_bottom", "bullish", &c2.ts, conf, vec![c1.clone(), c2.clone()]))
}

/// Tweezer top: matching highs after an up move, bull→bear pair.
fn tweezer_top(c: &[Candle], lookback: usize) -> Option<PatternResult> {
    let n = c.len();
    if n < lookback + 2 || trend_slope(&c[n - lookback - 2..n - 2]) <= 0.0 {
        return None;
    }
    let (c1, c2) = (&c[n - 2], &c[n - 1]);
    let tol = (c1.range() + c2.range()) / 2.0 * 0.1;
    if (c1.high - c2.high).abs() > tol || !c1.is_bull() || !c2.is_bear() {
        return None;
    }
    let conf = (0.6 + c2.body_ratio() * 0.4).min(1.0);
    Some(result("tweezer_top", "bearish", &c2.ts, conf, vec![c1.clone(), c2.clone()]))
}

/// Bullish marubozu: near-full body, tiny wicks (momentum impulse).
fn marubozu_bull(c: &[Candle], _lookback: usize) -> Option<PatternResult> {
    let last = c.last()?;
    if !last.is_bull() || last.body_ratio() < 0.90 {
        return None;
    }
    Some(result("marubozu_bull", "bullish", &last.ts, last.body_ratio(), vec![last.clone()]))
}

/// Bearish marubozu: near-full body, tiny wicks (momentum impulse).
fn marubozu_bear(c: &[Candle], _lookback: usize) -> Option<PatternResult> {
    let last = c.last()?;
    if !last.is_bear() || last.body_ratio() < 0.90 {
        return None;
    }
    Some(result("marubozu_bear", "bearish", &last.ts, last.body_ratio(), vec![last.clone()]))
}

/// All sixteen single-/few-bar detectors in priority order.
type Detector = fn(&[Candle], usize) -> Option<PatternResult>;
const DETECTORS: [Detector; 16] = [
    three_white_soldiers,
    morning_star,
    hammer,
    bullish_engulfing,
    three_black_crows,
    evening_star,
    hanging_man,
    bearish_engulfing,
    pin_bar_bull,
    pin_bar_bear,
    inside_bar_break_up,
    inside_bar_break_down,
    tweezer_bottom,
    tweezer_top,
    marubozu_bull,
    marubozu_bear,
];

// --------------------------------------------------------------------------
// modern overlays (section 9)
// --------------------------------------------------------------------------

/// Average True Range over the trailing `period` bars.
pub fn compute_atr(candles: &[Candle], period: usize) -> f64 {
    let n = candles.len();
    if n < period + 1 {
        return 0.0;
    }
    let mut trs = Vec::with_capacity(period);
    for i in 1..=period {
        let c = &candles[n - i];
        let p = &candles[n - i - 1];
        trs.push((c.high - c.low).max((c.high - p.close).abs()).max((c.low - p.close).abs()));
    }
    let m = trs.iter().sum::<f64>() / trs.len() as f64;
    if m == 0.0 { 1e-9 } else { m }
}

fn check_atr_threshold(candles: &[Candle], atr: f64, min_body_atr_ratio: f64) -> bool {
    !candles.is_empty() && atr > 0.0 && candles[candles.len() - 1].body() >= atr * min_body_atr_ratio
}

fn check_volume_spike(candles: &[Candle], period: usize, threshold: f64) -> bool {
    let n = candles.len();
    if n < period + 1 {
        return false;
    }
    let recent: Vec<f64> = candles[n - period - 1..n - 1].iter().map(|c| c.volume).collect();
    let avg = recent.iter().sum::<f64>() / recent.len() as f64;
    if avg <= 1e-9 {
        return false;
    }
    candles[n - 1].volume >= avg * threshold
}

/// Compute the composite score for a result given which sources were computed.
///
/// RULE/ATR/VOLUME always contribute; MTF/ML/GAF contribute only when the caller
/// actually computed them (`has_*`), otherwise they are excluded from the
/// denominator (see `StrategyConfig::composite`).
pub fn apply_strategy(r: &mut PatternResult, cfg: &StrategyConfig, has_mtf: bool, has_ml: bool, has_gaf: bool) {
    let signals: HashMap<Source, Option<f64>> = HashMap::from([
        (Source::Rule, Some(r.confidence)),
        (Source::Atr, Some(if r.atr_normalized { 1.0 } else { 0.0 })),
        (Source::Volume, Some(if r.volume_confirmed { 1.0 } else { 0.0 })),
        (Source::Mtf, has_mtf.then_some(r.mtf_score)),
        (Source::Ml, has_ml.then_some(r.ml_score)),
        (Source::Gaf, has_gaf.then_some(r.gaf_score)),
    ]);
    r.composite_score = cfg.composite(&signals);
}

/// Mean volume over the `lookback` bars *before* bar `i` (0 if insufficient).
fn avg_volume(candles: &[Candle], i: usize, lookback: usize) -> f64 {
    if lookback == 0 || i < lookback {
        return 0.0;
    }
    candles[i - lookback..i].iter().map(|c| c.volume).sum::<f64>() / lookback as f64
}

/// Detect context-aware day-trading setups (VWAP / ORB / EMA pullback) on the
/// closed bar `i`. Unlike the windowed [`PatternDetector`], these need the full
/// session history, supplied via the precomputed [`SessionContext`] and raw EMA
/// arrays (`NaN` warmup) so VWAP/opening-range anchors stay correct.
///
/// Each result carries its own ATR/volume confirmation flags so the caller can
/// run the same composite scoring and entry gates as the reversal patterns.
/// Bearish setups are emitted only when `allow_short` is set.
pub fn detect_setups(
    candles: &[Candle],
    i: usize,
    ctx: &SessionContext,
    ema9: &[f64],
    ema20: &[f64],
    enabled: &[String],
    allow_short: bool,
) -> Vec<PatternResult> {
    let mut out: Vec<PatternResult> = vec![];
    if i < 2 || i >= candles.len() {
        return out;
    }
    // Need a prior *same-session* bar — otherwise VWAP/price comparisons would
    // straddle the day boundary against the previous session's close.
    if ctx.bars_into_day(i) < 1 {
        return out;
    }
    let c = &candles[i];
    let p = &candles[i - 1];
    let vwap = ctx.vwap[i];
    let vwap_prev = ctx.vwap[i - 1];
    if !vwap.is_finite() || vwap <= 0.0 {
        return out;
    }

    let atr = compute_atr(&candles[..=i], 14);
    let vavg = avg_volume(candles, i, 10);
    let vol_spike = vavg > 0.0 && c.volume >= vavg * 1.3;
    let body_atr = atr > 0.0 && c.body() >= atr * 0.3;

    let want = |name: &str| enabled.iter().any(|e| e == name);
    let build = |name: &str, ptype: &str, conf: f64, used: Vec<Candle>| -> PatternResult {
        let mut r = result(name, ptype, &c.ts, conf, used);
        r.volume_confirmed = vol_spike;
        r.atr_normalized = body_atr;
        r.composite_score = r.confidence;
        r
    };

    // VWAP reclaim / loss — close crosses back over the session VWAP.
    if want("vwap_reclaim") && c.close > vwap && p.close < vwap_prev && c.is_bull() {
        let conf = 0.55 + c.body_ratio() * 0.3 + if vol_spike { 0.15 } else { 0.0 };
        out.push(build("vwap_reclaim", "bullish", conf, vec![p.clone(), c.clone()]));
    }
    if allow_short && want("vwap_loss") && c.close < vwap && p.close > vwap_prev && c.is_bear() {
        let conf = 0.55 + c.body_ratio() * 0.3 + if vol_spike { 0.15 } else { 0.0 };
        out.push(build("vwap_loss", "bearish", conf, vec![p.clone(), c.clone()]));
    }

    // VWAP bounce / reject — pullback to VWAP holds (above) or fails (below).
    if want("vwap_bounce") && c.close > vwap && c.low <= vwap * 1.0015 && c.is_bull() {
        out.push(build("vwap_bounce", "bullish", 0.5 + c.body_ratio() * 0.4, vec![c.clone()]));
    }
    if allow_short && want("vwap_reject") && c.close < vwap && c.high >= vwap * 0.9985 && c.is_bear() {
        out.push(build("vwap_reject", "bearish", 0.5 + c.body_ratio() * 0.4, vec![c.clone()]));
    }

    // Opening-range breakout / breakdown — first close beyond the box.
    let (orh, orl) = (ctx.or_high[i], ctx.or_low[i]);
    if want("orb_breakout") && orh.is_finite() && c.close > orh && p.close <= orh && c.is_bull() {
        let conf = 0.55 + if vol_spike { 0.25 } else { 0.0 } + c.body_ratio() * 0.2;
        out.push(build("orb_breakout", "bullish", conf, vec![c.clone()]));
    }
    if allow_short && want("orb_breakdown") && orl.is_finite() && c.close < orl && p.close >= orl && c.is_bear() {
        let conf = 0.55 + if vol_spike { 0.25 } else { 0.0 } + c.body_ratio() * 0.2;
        out.push(build("orb_breakdown", "bearish", conf, vec![c.clone()]));
    }

    // EMA pullback continuation — trend-aligned dip that resumes.
    let (e9, e20) = (ema9[i], ema20[i]);
    if e9.is_finite() && e20.is_finite() {
        if want("ema_pullback_long") && e9 > e20 && c.low <= e9 * 1.002 && c.close > e9 && c.is_bull() {
            out.push(build("ema_pullback_long", "bullish", 0.55 + c.body_ratio() * 0.35, vec![c.clone()]));
        }
        if allow_short && want("ema_pullback_short") && e9 < e20 && c.high >= e9 * 0.998 && c.close < e9 && c.is_bear() {
            out.push(build("ema_pullback_short", "bearish", 0.55 + c.body_ratio() * 0.35, vec![c.clone()]));
        }
    }
    out
}

/// Stateless pattern detector (mirrors the Python `PatternDetector` class).
#[derive(Clone, Default)]
pub struct PatternDetector;

impl PatternDetector {
    /// Scan the last `recent_bars` bars for patterns (spec section 4/9/10-1).
    ///
    /// Each detector only fires on the final bar of its window, so we slide the
    /// window end back `recent_bars` times to also catch patterns formed a few
    /// bars ago. `recent_bars = 1` reproduces "last bar only".
    pub fn scan(
        &self,
        candles: &[Candle],
        tf: Timeframe,
        min_confidence: f64,
        use_modern: bool,
        strategy: &StrategyConfig,
        recent_bars: usize,
    ) -> Vec<PatternResult> {
        let cfg = tf.config();
        if candles.len() < 3 {
            return vec![];
        }
        let vol_period = if tf != Timeframe::D1 { 10 } else { 20 };
        let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
        let mut results: Vec<PatternResult> = vec![];

        for offset in 0..recent_bars.max(1) {
            if candles.len() < offset + 3 {
                break;
            }
            let end = candles.len() - offset;
            if end < 3 {
                break;
            }
            let window = &candles[..end];
            let atr = if use_modern { compute_atr(window, 14) } else { 0.0 };
            for detector in DETECTORS {
                let Some(mut r) = detector(window, cfg.trend_lookback) else {
                    continue;
                };
                let key = (r.pattern_name.clone(), r.detected_at.clone());
                if seen.contains(&key) {
                    continue;
                }
                if use_modern {
                    r.atr_normalized = check_atr_threshold(window, atr, 0.3);
                    r.volume_confirmed = check_volume_spike(window, vol_period, 1.5);
                    // MTF needs upper-timeframe fetches, so it isn't computed in the
                    // synchronous scan; the trading engine fills mtf_score separately.
                    apply_strategy(&mut r, strategy, false, false, false);
                } else {
                    r.composite_score = r.confidence;
                }
                if r.confidence >= min_confidence {
                    seen.insert(key);
                    results.push(r);
                }
            }
        }
        results.sort_by(|a, b| b.composite_score.partial_cmp(&a.composite_score).unwrap_or(std::cmp::Ordering::Equal));
        results
    }
}
