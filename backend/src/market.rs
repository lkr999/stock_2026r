//! Market-context gate for the v2 strategies: broad-market regime + per-symbol
//! relative strength.
//!
//! The legacy path never looked at the market. Its only trend gate
//! (`require_higher_tf_uptrend`) inspected the *traded symbol's own* higher
//! timeframe, which is why a rising index still produced losses — entries kept
//! landing in names that were lagging the market instead of leading it.
//!
//! The market is read through a **listed proxy ETF** (KODEX 200 by default)
//! rather than an index code, so it flows through the exact same t8452 candle
//! path as any stock: no new API surface, no index-specific parsing, and the
//! proxy's candles land in the same TTL cache (one extra fetch per timeframe
//! per cycle at most).

use crate::candle::Candle;
use crate::candle_fetcher::CandleFetcher;
use crate::indicators::ema_values;
use crate::timeframe::Timeframe;

/// Bars of proxy history required before the regime is judged at all.
const MIN_BARS: usize = 25;
/// EMA the proxy must hold to count as risk-on.
const REGIME_EMA: usize = 20;

/// A read of the broad market at one point in time.
#[derive(Clone, Debug)]
pub struct MarketContext {
    /// Proxy is above its EMA20 **and** that EMA is not falling.
    pub risk_on: bool,
    pub above_ema: bool,
    /// EMA20 slope over the last 5 bars, as % of price (>0 = rising).
    pub ema_slope_pct: f64,
    /// Proxy return over the relative-strength lookback, in percent.
    pub ret_pct: f64,
    pub proxy_code: String,
    pub last_close: f64,
}

impl MarketContext {
    /// Neutral context used when the proxy can't be read (missing data, a bad
    /// proxy code). Fails **open**: a data outage must not silently halt
    /// trading, and it must not silently disable the RS gate either — `ret_pct`
    /// of 0 means the symbol is then judged on its own absolute return.
    pub fn unavailable(proxy_code: &str) -> Self {
        Self {
            risk_on: true,
            above_ema: true,
            ema_slope_pct: 0.0,
            ret_pct: 0.0,
            proxy_code: proxy_code.to_string(),
            last_close: 0.0,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "proxy_code": self.proxy_code,
            "risk_on": self.risk_on,
            "above_ema": self.above_ema,
            "ema_slope_pct": (self.ema_slope_pct * 1000.0).round() / 1000.0,
            "ret_pct": (self.ret_pct * 100.0).round() / 100.0,
            "last_close": self.last_close.round(),
        })
    }
}

/// Percent change of `closes` over the trailing `lookback` bars.
fn ret_over(candles: &[Candle], lookback: usize) -> f64 {
    if candles.len() < lookback + 1 || lookback == 0 {
        return 0.0;
    }
    let last = candles[candles.len() - 1].close;
    let first = candles[candles.len() - 1 - lookback].close;
    if first <= 0.0 {
        return 0.0;
    }
    (last - first) / first * 100.0
}

/// Judge the market from an already-fetched proxy candle series (closed bars).
///
/// Split out from [`assess`] so it is testable without a broker connection.
pub fn assess_candles(proxy_code: &str, closed: &[Candle], rs_lookback: usize) -> MarketContext {
    if closed.len() < MIN_BARS {
        return MarketContext::unavailable(proxy_code);
    }
    let ema = ema_values(closed, REGIME_EMA);
    let n = closed.len();
    let last_close = closed[n - 1].close;
    let ema_now = ema[n - 1];
    if !ema_now.is_finite() || ema_now <= 0.0 {
        return MarketContext::unavailable(proxy_code);
    }
    // Slope of the EMA itself over 5 bars, normalized to price — the EMA is
    // already smoothed, so this is a stable "is the market drifting up" read
    // that a single noisy bar cannot flip.
    let back = 5.min(n - 1);
    let ema_prev = ema[n - 1 - back];
    let ema_slope_pct = if ema_prev.is_finite() && ema_prev > 0.0 {
        (ema_now - ema_prev) / ema_prev * 100.0
    } else {
        0.0
    };
    let above_ema = last_close > ema_now;
    MarketContext {
        risk_on: above_ema && ema_slope_pct >= 0.0,
        above_ema,
        ema_slope_pct,
        ret_pct: ret_over(closed, rs_lookback),
        proxy_code: proxy_code.to_string(),
        last_close,
    }
}

/// Fetch the proxy and judge the market on the given timeframe.
///
/// The still-forming last bar is dropped — the regime is a closed-bar decision,
/// exactly like every entry decision it gates.
pub async fn assess(
    fetcher: &CandleFetcher,
    token: &str,
    proxy_code: &str,
    tf: Timeframe,
    rs_lookback: usize,
) -> MarketContext {
    let candles = fetcher.fetch(token, proxy_code, tf).await;
    if candles.len() < 2 {
        return MarketContext::unavailable(proxy_code);
    }
    assess_candles(proxy_code, &candles[..candles.len() - 1], rs_lookback)
}

/// Relative strength in percentage points: how much the symbol out-returned the
/// market proxy over `lookback` closed bars. Positive = leading the market.
pub fn relative_strength(symbol_closed: &[Candle], market: &MarketContext, lookback: usize) -> f64 {
    ret_over(symbol_closed, lookback) - market.ret_pct
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(closes: &[f64]) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| Candle {
                ts: format!("20260807 {:04}", 900 + i),
                date: "20260807".into(),
                time: format!("{:04}", 900 + i),
                open: c,
                high: c * 1.002,
                low: c * 0.998,
                close: c,
                volume: 1000.0,
            })
            .collect()
    }

    fn ramp(start: f64, step: f64, n: usize) -> Vec<f64> {
        (0..n).map(|i| start + step * i as f64).collect()
    }

    #[test]
    fn rising_proxy_is_risk_on_and_falling_is_not() {
        let up = assess_candles("069500", &series(&ramp(100.0, 0.5, 40)), 20);
        assert!(up.risk_on && up.above_ema && up.ema_slope_pct > 0.0);

        let down = assess_candles("069500", &series(&ramp(120.0, -0.5, 40)), 20);
        assert!(!down.risk_on && !down.above_ema);
    }

    #[test]
    fn short_history_fails_open_rather_than_halting_trading() {
        let ctx = assess_candles("069500", &series(&ramp(100.0, 0.5, 10)), 20);
        assert!(ctx.risk_on);
        assert_eq!(ctx.ret_pct, 0.0);
    }

    #[test]
    fn relative_strength_separates_leaders_from_laggards() {
        // 20 bars back from the last close: 119.5 vs 109.5 → +9.13%.
        let market = assess_candles("069500", &series(&ramp(100.0, 0.5, 40)), 20);
        assert!((market.ret_pct - 9.13).abs() < 0.05, "{}", market.ret_pct);

        let leader = series(&ramp(100.0, 1.0, 40)); // ~+25% over the same window
        let laggard = series(&ramp(100.0, 0.1, 40)); // ~+2%
        assert!(relative_strength(&leader, &market, 20) > 0.0);
        assert!(relative_strength(&laggard, &market, 20) < 0.0);
    }
}
