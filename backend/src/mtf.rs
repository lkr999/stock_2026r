//! Multi-timeframe confluence scoring (spec section 9-3).

use crate::candle_fetcher::CandleFetcher;
use crate::pattern::{trend_slope, PatternDetector};
use crate::strategy::StrategyConfig;
use crate::timeframe::Timeframe;

/// Scores how many higher timeframes confirm a base-timeframe signal.
pub struct MtfEngine<'a> {
    fetcher: &'a CandleFetcher,
    detector: &'a PatternDetector,
}

impl<'a> MtfEngine<'a> {
    pub fn new(fetcher: &'a CandleFetcher, detector: &'a PatternDetector) -> Self {
        Self { fetcher, detector }
    }

    /// Fraction of upper timeframes that show a same-direction signal (0.5 if none).
    pub async fn score(&self, token: &str, shcode: &str, base_tf: Timeframe, pattern_type: &str) -> f64 {
        let uppers = base_tf.mtf_group();
        if uppers.is_empty() {
            return 0.5; // neutral when no higher timeframe exists
        }
        let mut hits = 0;
        let default_cfg = StrategyConfig::default();
        for tf in &uppers {
            let candles = self.fetcher.fetch(token, shcode, *tf).await;
            // Judge on closed bars only — the last bar is still forming and its
            // shape can flip before the close.
            let closed = if candles.is_empty() { &candles[..] } else { &candles[..candles.len() - 1] };
            let results = self.detector.scan(closed, *tf, 0.5, true, &default_cfg, 1);
            if results.iter().any(|r| r.pattern_type == pattern_type) {
                hits += 1;
            }
        }
        hits as f64 / uppers.len() as f64
    }

    /// True unless the nearest higher timeframe is in a downtrend (long-entry gate).
    ///
    /// `slope_tolerance` relaxes the gate: the trend counts as "not down" while
    /// the per-bar slope, normalized by the mean price, stays above −tolerance
    /// (so 0.0005 allows a drift down to −0.05%/bar). 0 = strict (legacy).
    pub async fn higher_tf_uptrend(&self, token: &str, shcode: &str, base_tf: Timeframe, slope_tolerance: f64) -> bool {
        let uppers = base_tf.mtf_group();
        let Some(upper) = uppers.first() else {
            return true;
        };
        let candles = self.fetcher.fetch(token, shcode, *upper).await;
        // Drop the still-forming last bar — trend must be judged on closed bars.
        if candles.len() < 6 {
            return true;
        }
        let closed = &candles[..candles.len() - 1];
        let tail = &closed[closed.len().saturating_sub(20)..];
        let mean = tail.iter().map(|c| c.close).sum::<f64>() / tail.len() as f64;
        if mean <= 0.0 {
            return true;
        }
        trend_slope(tail) / mean >= -slope_tolerance.max(0.0)
    }
}
