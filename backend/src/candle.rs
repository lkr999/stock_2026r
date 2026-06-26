//! Unified OHLCV candle + candlestick geometry helpers (spec section 4).

use serde::{Deserialize, Serialize};

/// One OHLCV bar. `ts`/`date`/`time` carry the eBest timestamps used by the chart.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Candle {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub time: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    #[serde(default)]
    pub volume: f64,
}

impl Candle {
    /// Absolute candle body size |close − open|.
    pub fn body(&self) -> f64 {
        (self.close - self.open).abs()
    }
    /// High−low range (floored to avoid divide-by-zero).
    pub fn range(&self) -> f64 {
        let r = self.high - self.low;
        if r == 0.0 { 1e-9 } else { r }
    }
    /// Body as a fraction of the full range.
    pub fn body_ratio(&self) -> f64 {
        self.body() / self.range()
    }
    /// Upper wick length.
    pub fn upper_shadow(&self) -> f64 {
        self.high - self.open.max(self.close)
    }
    /// Lower wick length.
    pub fn lower_shadow(&self) -> f64 {
        self.open.min(self.close) - self.low
    }
    /// Bullish (or doji) candle.
    pub fn is_bull(&self) -> bool {
        self.close >= self.open
    }
    /// Bearish candle.
    pub fn is_bear(&self) -> bool {
        self.close < self.open
    }
}
