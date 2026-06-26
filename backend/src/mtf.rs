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
            let results = self.detector.scan(&candles, *tf, 0.5, true, &default_cfg, 1);
            if results.iter().any(|r| r.pattern_type == pattern_type) {
                hits += 1;
            }
        }
        hits as f64 / uppers.len() as f64
    }

    /// True unless the nearest higher timeframe is in a downtrend (long-entry gate).
    pub async fn higher_tf_uptrend(&self, token: &str, shcode: &str, base_tf: Timeframe) -> bool {
        let uppers = base_tf.mtf_group();
        let Some(upper) = uppers.first() else {
            return true;
        };
        let candles = self.fetcher.fetch(token, shcode, *upper).await;
        if candles.len() < 5 {
            return true;
        }
        let tail = &candles[candles.len().saturating_sub(20)..];
        trend_slope(tail) >= 0.0
    }
}
