//! Caginalp & Laurent (1998) reversal-pattern detector + modern overlays (sections 4, 9).
//!
//! The system is long-only, so only bullish (매수 진입) patterns are detected.
//! Every rule is deterministic. ATR-adaptive thresholds and volume confirmation
//! (section 9) are applied as overlays; the composite score is computed via
//! `StrategyConfig` (section 10-1), not hardcoded.

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
        _ => (0.0, 0.0, 0.0),
    }
}

/// Every single-/few-bar pattern name (used by the batch backtest set).
pub const ALL_PATTERNS: [&str; 8] = [
    "three_white_soldiers",
    "morning_star",
    "hammer",
    "bullish_engulfing",
    "pin_bar_bull",
    "inside_bar_break_up",
    "tweezer_bottom",
    "marubozu_bull",
];

/// Context-aware day-trading setup names (VWAP / ORB / EMA pullback / RSI
/// mean-reversion / Bollinger squeeze breakout).
/// These are detected by [`detect_setups`], not the windowed [`PatternDetector`].
pub const SETUP_PATTERNS: [&str; 6] = [
    "vwap_reclaim",
    "vwap_bounce",
    "orb_breakout",
    "ema_pullback",
    "rsi_oversold_bounce",
    "bb_squeeze_break_up",
];

/// v2 setup names, detected by [`detect_v2_setups`].
///
/// Unlike the legacy setups these embed their own quality tests (volume ratio,
/// body-to-ATR, close position in range, trend alignment) *inside* the detector
/// instead of leaving them to a composite score that in practice passed almost
/// everything. A v2 setup that fires has already cleared its filters.
pub const V2_SETUP_PATTERNS: [&str; 5] = [
    "v2_trend_pullback",
    "v2_power_breakout",
    "v2_orb_retest",
    "v2_rsi_reversion",
    "v2_squeeze_expansion",
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

fn result(name: &str, ts: &str, conf: f64, used: Vec<Candle>) -> PatternResult {
    let (e5, e10, e25) = paper_returns(name);
    PatternResult {
        pattern_name: name.into(),
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
// bullish reversal detectors — each inspects the *last* bar(s) of the window
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
    Some(result("three_white_soldiers", &c3.ts, conf, vec![c1.clone(), c2.clone(), c3.clone()]))
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
    Some(result("morning_star", &c3.ts, conf, vec![c1.clone(), c2.clone(), c3.clone()]))
}

/// Hammer geometry test → (valid, confidence).
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
    valid.then(|| result("hammer", &last.ts, conf, vec![last.clone()]))
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
    Some(result("bullish_engulfing", &c2.ts, conf, vec![c1.clone(), c2.clone()]))
}

// --------------------------------------------------------------------------
// intraday candlestick patterns (단타 보강) — pin bar / inside-bar break /
// tweezer / marubozu. Single-/few-bar geometry, so they slot into the same
// windowed detector interface as the reversal patterns above.
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
    Some(result("pin_bar_bull", &last.ts, conf, vec![last.clone()]))
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
    Some(result("inside_bar_break_up", &brk.ts, conf, vec![mother.clone(), inside.clone(), brk.clone()]))
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
    Some(result("tweezer_bottom", &c2.ts, conf, vec![c1.clone(), c2.clone()]))
}

/// Bullish marubozu: near-full body, tiny wicks (momentum impulse).
fn marubozu_bull(c: &[Candle], _lookback: usize) -> Option<PatternResult> {
    let last = c.last()?;
    if !last.is_bull() || last.body_ratio() < 0.90 {
        return None;
    }
    Some(result("marubozu_bull", &last.ts, last.body_ratio(), vec![last.clone()]))
}

/// All eight single-/few-bar detectors in priority order.
type Detector = fn(&[Candle], usize) -> Option<PatternResult>;
const DETECTORS: [Detector; 8] = [
    three_white_soldiers,
    morning_star,
    hammer,
    bullish_engulfing,
    pin_bar_bull,
    inside_bar_break_up,
    tweezer_bottom,
    marubozu_bull,
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

/// Precomputed indicator arrays the context setups read (`NaN` = warmup).
/// Built once per scan via [`SetupSeries::compute`] so the per-bar setup checks
/// stay O(1) — the engine and the backtester share the exact same inputs.
pub struct SetupSeries {
    pub ema9: Vec<f64>,
    pub ema20: Vec<f64>,
    /// Slower trend anchor — the v2 setups require EMA20 > EMA50 alignment
    /// before buying any dip, which is what separates a pullback from a
    /// falling knife.
    pub ema50: Vec<f64>,
    pub rsi14: Vec<f64>,
    pub bb_upper: Vec<f64>,
    /// Relative band width (upper−lower)/mid — the squeeze measure.
    pub bb_width: Vec<f64>,
}

impl SetupSeries {
    pub fn compute(candles: &[Candle]) -> Self {
        let (bb_upper, _bb_lower, bb_width) = crate::indicators::bollinger_values(candles, 20, 2.0);
        Self {
            ema9: crate::indicators::ema_values(candles, 9),
            ema20: crate::indicators::ema_values(candles, 20),
            ema50: crate::indicators::ema_values(candles, 50),
            rsi14: crate::indicators::rsi_values(candles, 14),
            bb_upper,
            bb_width,
        }
    }
}

/// Detect context-aware day-trading setups (VWAP / ORB / EMA pullback / RSI
/// mean-reversion / Bollinger squeeze) on the closed bar `i`. Unlike the
/// windowed [`PatternDetector`], these need the full session history, supplied
/// via the precomputed [`SessionContext`] and [`SetupSeries`] (`NaN` warmup) so
/// VWAP/opening-range anchors stay correct.
///
/// Each result carries its own ATR/volume confirmation flags so the caller can
/// run the same composite scoring and entry gates as the reversal patterns.
pub fn detect_setups(
    candles: &[Candle],
    i: usize,
    ctx: &SessionContext,
    series: &SetupSeries,
    enabled: &[String],
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
    let build = |name: &str, conf: f64, used: Vec<Candle>| -> PatternResult {
        let mut r = result(name, &c.ts, conf, used);
        r.volume_confirmed = vol_spike;
        r.atr_normalized = body_atr;
        r.composite_score = r.confidence;
        r
    };

    // VWAP reclaim — close crosses back above the session VWAP.
    if want("vwap_reclaim") && c.close > vwap && p.close < vwap_prev && c.is_bull() {
        let conf = 0.55 + c.body_ratio() * 0.3 + if vol_spike { 0.15 } else { 0.0 };
        out.push(build("vwap_reclaim", conf, vec![p.clone(), c.clone()]));
    }

    // VWAP bounce — pullback to VWAP holds from above.
    if want("vwap_bounce") && c.close > vwap && c.low <= vwap * 1.0015 && c.is_bull() {
        out.push(build("vwap_bounce", 0.5 + c.body_ratio() * 0.4, vec![c.clone()]));
    }

    // Opening-range breakout — first close above the box.
    let orh = ctx.or_high[i];
    if want("orb_breakout") && orh.is_finite() && c.close > orh && p.close <= orh && c.is_bull() {
        let conf = 0.55 + if vol_spike { 0.25 } else { 0.0 } + c.body_ratio() * 0.2;
        out.push(build("orb_breakout", conf, vec![c.clone()]));
    }

    // EMA pullback continuation — uptrend-aligned dip that resumes.
    let (e9, e20) = (series.ema9[i], series.ema20[i]);
    if e9.is_finite() && e20.is_finite() && want("ema_pullback") && e9 > e20 && c.low <= e9 * 1.002 && c.close > e9 && c.is_bull() {
        out.push(build("ema_pullback", 0.55 + c.body_ratio() * 0.35, vec![c.clone()]));
    }

    // RSI mean-reversion — RSI(14) leaves the oversold zone with a confirming
    // bar (Connors-style; historically among the highest win-rate intraday
    // setups because it buys exhaustion instead of chasing).
    let (rsi_prev, rsi_now) = (series.rsi14[i - 1], series.rsi14[i]);
    if rsi_prev.is_finite() && rsi_now.is_finite()
        && want("rsi_oversold_bounce") && rsi_prev <= 30.0 && rsi_now > rsi_prev && c.is_bull()
    {
        // Deeper oversold → stronger snap-back expectancy.
        let depth = ((30.0 - rsi_prev) / 30.0).clamp(0.0, 1.0);
        let conf = 0.5 + depth * 0.2 + c.body_ratio() * 0.15 + if vol_spike { 0.15 } else { 0.0 };
        out.push(build("rsi_oversold_bounce", conf, vec![p.clone(), c.clone()]));
    }

    // Bollinger squeeze breakout — volatility contraction (band width near its
    // 20-bar low) resolving with a close above the upper band. Squeezes precede
    // expansions; entering on the break rides the new impulse early.
    let (bb_u, bw) = (series.bb_upper[i], series.bb_width[i - 1]);
    if bb_u.is_finite() && bw.is_finite() && i >= 21 {
        let lookback = &series.bb_width[i.saturating_sub(20)..i];
        let min_w = lookback.iter().copied().filter(|w| w.is_finite()).fold(f64::INFINITY, f64::min);
        let squeezed = min_w.is_finite() && bw <= min_w * 1.10; // was at/near the tightest width
        if squeezed && want("bb_squeeze_break_up") && c.close > bb_u && c.is_bull() {
            let conf = 0.55 + if vol_spike { 0.25 } else { 0.0 } + c.body_ratio() * 0.2;
            out.push(build("bb_squeeze_break_up", conf, vec![c.clone()]));
        }
    }
    out
}

// --------------------------------------------------------------------------
// v2 setups — quality-gated redesign
// --------------------------------------------------------------------------

/// Where the close sits inside the bar's range (1.0 = closed on the high).
/// A breakout that closes mid-bar has already been sold into; requiring an
/// upper-range close is one of the cheapest win-rate filters available.
fn close_position(c: &Candle) -> f64 {
    ((c.close - c.low) / c.range()).clamp(0.0, 1.0)
}

/// Highest high over the `n` bars *before* bar `i`.
fn prior_high(candles: &[Candle], i: usize, n: usize) -> f64 {
    if i < n || n == 0 {
        return f64::NAN;
    }
    candles[i - n..i].iter().map(|c| c.high).fold(f64::MIN, f64::max)
}

/// Detect the v2 day-trading setups on the closed bar `i`.
///
/// Mirrors [`detect_setups`]'s contract (same inputs, same `PatternResult`
/// output, same `enabled` filtering) so the engine and the backtester can run
/// both generations through one code path. The difference is what gets past the
/// door: each detector here applies trend alignment, a volume ratio, a
/// body-to-ATR floor and a close-position test before it will fire at all.
pub fn detect_v2_setups(
    candles: &[Candle],
    i: usize,
    ctx: &SessionContext,
    series: &SetupSeries,
    enabled: &[String],
) -> Vec<PatternResult> {
    let mut out: Vec<PatternResult> = vec![];
    // EMA50 warmup dominates the requirement; below this nothing is trustworthy.
    if i < 51 || i >= candles.len() {
        return out;
    }
    let want = |name: &str| enabled.iter().any(|e| e == name);
    if !V2_SETUP_PATTERNS.iter().any(|n| want(n)) {
        return out;
    }

    let c = &candles[i];
    let p = &candles[i - 1];
    let atr = compute_atr(&candles[..=i], 14);
    if atr <= 0.0 {
        return out;
    }
    let vavg = avg_volume(candles, i, 20);
    let vol_ratio = if vavg > 0.0 { c.volume / vavg } else { 0.0 };
    let body_atr = c.body() / atr;
    let cpos = close_position(c);
    let (e20, e50) = (series.ema20[i], series.ema50[i]);
    if !e20.is_finite() || !e50.is_finite() || e20 <= 0.0 || e50 <= 0.0 {
        return out;
    }
    let uptrend = e20 > e50;

    let build = |name: &str, conf: f64, used: Vec<Candle>| -> PatternResult {
        let mut r = result(name, &c.ts, conf, used);
        // The detector already enforced its own thresholds, so these flags
        // report the *measured* quality rather than acting as the gate.
        r.volume_confirmed = vol_ratio >= 1.5;
        r.atr_normalized = body_atr >= 0.5;
        r.composite_score = r.confidence;
        r
    };

    // -- 추세 눌림목 --------------------------------------------------------
    // EMA20 > EMA50 정렬 상태에서 EMA20 까지 눌린 뒤 다시 위로 마감. 기존
    // ema_pullback 은 EMA9/EMA20 만 봐서 하락 추세에서도 발동했다(승률 20%).
    if want("v2_trend_pullback") && uptrend && c.is_bull() {
        let touched = c.low <= e20 * 1.004 || p.low <= e20 * 1.004;
        let reclaimed = c.close > e20 && c.close > p.close;
        // 추세가 살아있다는 최소 조건: EMA20 이 EMA50 위로 뚜렷하게 벌어져 있을 것.
        let separation = (e20 - e50) / e50;
        if touched && reclaimed && separation >= 0.001 {
            let conf = 0.58
                + (separation * 40.0).clamp(0.0, 0.12)
                + (cpos - 0.5).max(0.0) * 0.20
                + if vol_ratio >= 1.2 { 0.08 } else { 0.0 };
            out.push(build("v2_trend_pullback", conf, vec![p.clone(), c.clone()]));
        }
    }

    // -- 정밀 돌파 ----------------------------------------------------------
    // 20봉 신고가 + 거래량 2배 + 몸통 0.6ATR + 상단 70% 마감. 네 조건을 모두
    // 통과한 돌파만 — 기존 orb_breakout/marubozu 류의 맨몸 돌파를 대체한다.
    if want("v2_power_breakout") && c.is_bull() {
        let hh = prior_high(candles, i, 20);
        if hh.is_finite()
            && c.close > hh
            && vol_ratio >= 2.0
            && body_atr >= 0.6
            && cpos >= 0.7
            && c.close > e20
        {
            let conf = 0.62
                + ((vol_ratio - 2.0) / 4.0).clamp(0.0, 0.12)
                + ((body_atr - 0.6) / 2.0).clamp(0.0, 0.10)
                + if uptrend { 0.06 } else { 0.0 };
            out.push(build("v2_power_breakout", conf, vec![c.clone()]));
        }
    }

    // -- 오프닝레인지 리테스트 ------------------------------------------------
    // 돌파를 추격하지 않고, 돌파 후 박스 상단까지 되돌아와 지지받는 자리를 산다.
    // 손절이 박스 상단 바로 아래라 리스크가 명확하고 짧다.
    let orh = ctx.or_high[i];
    if want("v2_orb_retest") && orh.is_finite() && c.is_bull() {
        let day_start = i - ctx.bars_into_day(i);
        // 이미 이번 세션에 박스를 뚫은 적이 있어야 "리테스트"다.
        let broke_earlier = (day_start..i).any(|j| candles[j].close > orh);
        let retested = c.low <= orh * 1.004 && c.close > orh;
        if broke_earlier && retested && c.close > p.close && cpos >= 0.55 {
            let conf = 0.60
                + (cpos - 0.55).max(0.0) * 0.25
                + if vol_ratio >= 1.3 { 0.10 } else { 0.0 }
                + if uptrend { 0.05 } else { 0.0 };
            out.push(build("v2_orb_retest", conf, vec![p.clone(), c.clone()]));
        }
    }

    // -- 과매도 반등 (추세 필터 부착) -------------------------------------------
    // 기존 rsi_oversold_bounce 와의 결정적 차이는 EMA50 위에서만 발동한다는 것.
    // 평균회귀는 상승 추세 안의 눌림에서만 통계적 우위가 있다.
    let (rsi_prev, rsi_now) = (series.rsi14[i - 1], series.rsi14[i]);
    if want("v2_rsi_reversion")
        && rsi_prev.is_finite()
        && rsi_now.is_finite()
        && c.is_bull()
        && c.close > e50            // 상승 추세 안에서만
        && rsi_prev <= 35.0         // 소진 구간
        && rsi_now > rsi_prev       // 반등 시작
        && c.close > p.close
    {
        let depth = ((35.0 - rsi_prev) / 35.0).clamp(0.0, 1.0);
        let conf = 0.58
            + depth * 0.18
            + (cpos - 0.5).max(0.0) * 0.16
            + if uptrend { 0.06 } else { 0.0 };
        out.push(build("v2_rsi_reversion", conf, vec![p.clone(), c.clone()]));
    }

    // -- 변동성 수축 → 확장 ---------------------------------------------------
    // 기존 bb_squeeze_break_up 에 거래량·추세 조건을 더했다 (기존은 3건 중
    // 1승, 평균 −3.76%).
    if want("v2_squeeze_expansion") && i >= 21 && c.is_bull() {
        let (bb_u, bw_prev) = (series.bb_upper[i], series.bb_width[i - 1]);
        if bb_u.is_finite() && bw_prev.is_finite() {
            let lookback = &series.bb_width[i - 20..i];
            let min_w = lookback.iter().copied().filter(|w| w.is_finite()).fold(f64::INFINITY, f64::min);
            let squeezed = min_w.is_finite() && bw_prev <= min_w * 1.10;
            if squeezed && c.close > bb_u && vol_ratio >= 1.8 && c.close > e20 && cpos >= 0.6 {
                let conf = 0.62
                    + ((vol_ratio - 1.8) / 4.0).clamp(0.0, 0.10)
                    + (cpos - 0.6).max(0.0) * 0.15
                    + if uptrend { 0.06 } else { 0.0 };
                out.push(build("v2_squeeze_expansion", conf, vec![c.clone()]));
            }
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

#[cfg(test)]
mod v2_tests {
    use super::*;

    /// Build a session of bars from (close, volume) pairs, with a small body
    /// and a close near the high unless `strong` widens it.
    fn bars(pairs: &[(f64, f64)]) -> Vec<Candle> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, &(c, v))| {
                let open = if i == 0 { c } else { pairs[i - 1].0 };
                Candle {
                    ts: format!("20260807{:04}", 900 + i),
                    date: "20260807".into(),
                    time: format!("{:04}", 900 + i),
                    open,
                    high: c.max(open) * 1.001,
                    low: c.min(open) * 0.999,
                    close: c,
                    volume: v,
                }
            })
            .collect()
    }

    /// A quiet uptrend long enough to warm up EMA50, then one decisive bar.
    fn uptrend_then(last_close: f64, last_vol: f64) -> Vec<Candle> {
        let mut p: Vec<(f64, f64)> = (0..60).map(|i| (100.0 + i as f64 * 0.2, 1000.0)).collect();
        p.push((last_close, last_vol));
        bars(&p)
    }

    fn detect(candles: &[Candle], want: &[&str]) -> Vec<String> {
        let i = candles.len() - 1;
        let ctx = SessionContext::build(candles, 3);
        let series = SetupSeries::compute(candles);
        let enabled: Vec<String> = want.iter().map(|s| s.to_string()).collect();
        detect_v2_setups(candles, i, &ctx, &series, &enabled)
            .into_iter()
            .map(|r| r.pattern_name)
            .collect()
    }

    #[test]
    fn power_breakout_needs_volume_body_and_a_strong_close() {
        // 20-bar high is ~111.8; close well above it on 5× volume.
        let fired = detect(&uptrend_then(118.0, 5000.0), &["v2_power_breakout"]);
        assert_eq!(fired, vec!["v2_power_breakout"]);

        // Identical price action on ordinary volume must not fire.
        let quiet = detect(&uptrend_then(118.0, 1000.0), &["v2_power_breakout"]);
        assert!(quiet.is_empty(), "{quiet:?}");
    }

    #[test]
    fn nothing_fires_for_patterns_the_strategy_has_not_enabled() {
        let fired = detect(&uptrend_then(118.0, 5000.0), &["v2_rsi_reversion"]);
        assert!(fired.is_empty(), "{fired:?}");
    }

    #[test]
    fn rsi_reversion_refuses_to_catch_a_falling_knife() {
        // A sustained downtrend: RSI is deeply oversold and ticks up on the
        // last bar, but price is far below EMA50 — the legacy detector would
        // have bought this, the v2 one must not.
        let mut p: Vec<(f64, f64)> = (0..60).map(|i| (140.0 - i as f64 * 0.6, 1000.0)).collect();
        p.push((105.0, 2000.0)); // green bar off the low
        let fired = detect(&bars(&p), &["v2_rsi_reversion"]);
        assert!(fired.is_empty(), "{fired:?}");
    }

    #[test]
    fn trend_pullback_requires_ema_alignment() {
        // 60 bars of +0.5/bar puts the close at 129.5 with EMA20 lagging near
        // 124.8 and EMA50 far below it. Dip through the EMA20, then reclaim.
        let mut p: Vec<(f64, f64)> = (0..60).map(|i| (100.0 + i as f64 * 0.5, 1000.0)).collect();
        p.push((120.0, 900.0)); // pull back below EMA20
        p.push((127.0, 1400.0)); // reclaim above it on rising volume
        let fired = detect(&bars(&p), &["v2_trend_pullback"]);
        assert_eq!(fired, vec!["v2_trend_pullback"]);

        // The same dip-and-bounce in a *downtrend* (EMA20 < EMA50) must not fire.
        let mut d: Vec<(f64, f64)> = (0..60).map(|i| (130.0 - i as f64 * 0.5, 1000.0)).collect();
        d.push((97.0, 900.0));
        d.push((104.0, 1400.0));
        assert!(detect(&bars(&d), &["v2_trend_pullback"]).is_empty());
    }

    #[test]
    fn warmup_is_respected_so_no_setup_fires_on_a_short_series() {
        let short = bars(&(0..40).map(|i| (100.0 + i as f64, 5000.0)).collect::<Vec<_>>());
        let fired = detect(&short, &V2_SETUP_PATTERNS);
        assert!(fired.is_empty(), "{fired:?}");
    }
}
