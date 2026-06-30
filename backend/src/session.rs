//! Intraday session context — per-day VWAP + opening range (단타 전략 기반).
//!
//! Day-trading setups (VWAP reclaim/bounce, opening-range breakout) need a
//! *session-anchored* reference, not a rolling window: VWAP and the opening
//! range reset at the first bar of each trading day. We precompute both as
//! per-bar series so the engine and the backtester can look them up by index
//! without re-deriving the session each call.

use crate::candle::Candle;
use crate::timeframe::Timeframe;

/// Per-bar session reference values, aligned 1:1 with the input candles.
pub struct SessionContext {
    /// Session VWAP known as of bar `i` (volume-weighted typical price).
    pub vwap: Vec<f64>,
    /// Opening-range high known as of bar `i` (`NaN` until the range completes).
    pub or_high: Vec<f64>,
    /// Opening-range low known as of bar `i` (`NaN` until the range completes).
    pub or_low: Vec<f64>,
    /// Index of the first bar of bar `i`'s session (day-start anchor).
    pub day_start: Vec<usize>,
}

/// Opening-range length (bars) per timeframe — ~15 minutes of the session.
pub fn or_bars_for(tf: Timeframe) -> usize {
    match tf {
        Timeframe::M1 => 15,
        Timeframe::M3 => 5,
        Timeframe::M5 => 3,
        Timeframe::M10 | Timeframe::M15 => 1,
        _ => 1,
    }
}

impl SessionContext {
    /// Build the session series. A new day starts whenever `candle.date` changes
    /// (daily candles, with an empty `time`, degenerate to one bar per day — the
    /// day-trading setups are simply never enabled there).
    pub fn build(candles: &[Candle], or_bars: usize) -> Self {
        let n = candles.len();
        let mut vwap = vec![0.0; n];
        let mut or_high = vec![f64::NAN; n];
        let mut or_low = vec![f64::NAN; n];
        let mut day_start = vec![0usize; n];

        let or_len = or_bars.max(1);
        let mut start = 0usize;
        let (mut cum_pv, mut cum_v) = (0.0, 0.0);
        let (mut cur_hi, mut cur_lo) = (f64::MIN, f64::MAX);

        for i in 0..n {
            let new_day = i == 0 || candles[i].date != candles[i - 1].date;
            if new_day {
                start = i;
                cum_pv = 0.0;
                cum_v = 0.0;
                cur_hi = f64::MIN;
                cur_lo = f64::MAX;
            }
            day_start[i] = start;

            let c = &candles[i];
            let tp = (c.high + c.low + c.close) / 3.0;
            // Some feeds report 0 volume on thin bars; fall back to equal weight.
            let v = if c.volume > 0.0 { c.volume } else { 1.0 };
            cum_pv += tp * v;
            cum_v += v;
            vwap[i] = if cum_v > 0.0 { cum_pv / cum_v } else { c.close };

            let bars_into_day = i - start; // 0-based position within the session
            if bars_into_day < or_len {
                cur_hi = cur_hi.max(c.high);
                cur_lo = cur_lo.min(c.low);
            }
            // The opening range is only *known* once its window has fully formed.
            if bars_into_day + 1 >= or_len {
                or_high[i] = cur_hi;
                or_low[i] = cur_lo;
            }
        }
        Self { vwap, or_high, or_low, day_start }
    }

    /// Convenience: build with the timeframe's default opening-range length.
    pub fn for_tf(candles: &[Candle], tf: Timeframe) -> Self {
        Self::build(candles, or_bars_for(tf))
    }

    /// Number of bars elapsed in bar `i`'s session (0 = session open).
    pub fn bars_into_day(&self, i: usize) -> usize {
        i - self.day_start[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(date: &str, o: f64, h: f64, l: f64, c: f64, v: f64) -> Candle {
        Candle { ts: date.into(), date: date.into(), time: String::new(), open: o, high: h, low: l, close: c, volume: v }
    }

    #[test]
    fn vwap_resets_each_day() {
        let candles = vec![
            bar("20240101", 10.0, 10.0, 10.0, 10.0, 100.0),
            bar("20240101", 20.0, 20.0, 20.0, 20.0, 100.0),
            bar("20240102", 50.0, 50.0, 50.0, 50.0, 100.0),
        ];
        let ctx = SessionContext::build(&candles, 1);
        assert!((ctx.vwap[0] - 10.0).abs() < 1e-9);
        assert!((ctx.vwap[1] - 15.0).abs() < 1e-9); // mean of day 1
        assert!((ctx.vwap[2] - 50.0).abs() < 1e-9); // day 2 reset
        assert_eq!(ctx.day_start, vec![0, 0, 2]);
        assert_eq!(ctx.bars_into_day(1), 1);
        assert_eq!(ctx.bars_into_day(2), 0);
    }

    #[test]
    fn opening_range_known_after_window() {
        let candles = vec![
            bar("20240101", 10.0, 12.0, 9.0, 11.0, 100.0),
            bar("20240101", 11.0, 15.0, 8.0, 14.0, 100.0),
            bar("20240101", 14.0, 16.0, 13.0, 15.0, 100.0),
        ];
        let ctx = SessionContext::build(&candles, 2); // first 2 bars form the range
        assert!(ctx.or_high[0].is_nan()); // range not complete yet
        assert_eq!(ctx.or_high[1], 12.0_f64.max(15.0)); // 15
        assert_eq!(ctx.or_low[1], 9.0_f64.min(8.0)); // 8
        assert_eq!(ctx.or_high[2], 15.0); // frozen after the window
        assert_eq!(ctx.or_low[2], 8.0);
    }
}
