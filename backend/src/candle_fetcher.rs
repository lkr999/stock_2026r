//! Timeframe-aware OHLCV fetcher wrapping `EBestService` (spec section 2-3).
//!
//! One `fetch()` routes daily candles to t8451 and intraday candles to t8452,
//! normalising both into the unified `Candle` shape, with a short TTL cache to
//! soften the eBest ~1 call/sec rate limit during universe scans.

use crate::candle::Candle;
use crate::ebest::{parse_float, EBestService};
use crate::timeframe::Timeframe;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Caches normalized candles per (code, timeframe) with a per-timeframe TTL.
pub struct CandleFetcher {
    ebest: Arc<EBestService>,
    cache: Mutex<HashMap<(String, Timeframe), (Instant, Vec<Candle>)>>,
}

impl CandleFetcher {
    pub fn new(ebest: Arc<EBestService>) -> Self {
        Self { ebest, cache: Mutex::new(HashMap::new()) }
    }

    /// Shorter TTL for fast timeframes; longer for daily.
    fn cache_ttl(tf: Timeframe) -> Duration {
        Duration::from_secs(match tf {
            Timeframe::M1 => 35,
            Timeframe::M3 | Timeframe::M5 => 90,
            Timeframe::D1 => 600,
            _ => 180,
        })
    }

    /// Fetch candles (ascending) for `shcode` at timeframe `tf`, using the cache.
    pub async fn fetch(&self, token: &str, shcode: &str, tf: Timeframe) -> Vec<Candle> {
        let key = (shcode.to_string(), tf);
        {
            let cache = self.cache.lock().await;
            if let Some((expiry, rows)) = cache.get(&key) {
                if Instant::now() < *expiry {
                    return rows.clone();
                }
            }
        }
        let rows = self.fetch_uncached(token, shcode, tf).await;
        let mut cache = self.cache.lock().await;
        cache.insert(key, (Instant::now() + Self::cache_ttl(tf), rows.clone()));
        rows
    }

    async fn fetch_uncached(&self, token: &str, shcode: &str, tf: Timeframe) -> Vec<Candle> {
        let cfg = tf.config();
        if tf == Timeframe::D1 {
            let raw = self.ebest.fetch_daily_candles(token, shcode, cfg.qrycnt).await;
            return raw.iter().map(Self::daily_row).collect();
        }
        let raw = self.ebest.fetch_minute_candles(token, shcode, cfg.ncnt, cfg.qrycnt).await;
        Self::normalize_minute(&raw)
    }

    /// Daily t8451 row → `Candle` (date only, no intraday time).
    fn daily_row(r: &Value) -> Candle {
        let f = |k: &str| r.get(k).and_then(parse_float).unwrap_or(0.0);
        let date = r.get("date").and_then(Value::as_str).unwrap_or("").to_string();
        Candle {
            ts: date.clone(),
            date,
            time: String::new(),
            open: f("open"),
            high: f("high"),
            low: f("low"),
            close: f("close"),
            volume: f("volume"),
        }
    }

    /// Normalize the t8452 OutBlock1 into ascending `Candle`s (skips bad bars).
    fn normalize_minute(raw: &Value) -> Vec<Candle> {
        let mut rows: Vec<Candle> = vec![];
        if let Some(items) = raw.get("t8452OutBlock1").and_then(Value::as_array) {
            for it in items {
                let f = |k: &str| it.get(k).and_then(parse_float).unwrap_or(0.0);
                let date = it.get("date").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let time = it.get("time").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let (high, low) = (f("high"), f("low"));
                if high > 0.0 && low > 0.0 {
                    rows.push(Candle {
                        ts: format!("{date} {time}").trim().to_string(),
                        date,
                        time,
                        open: f("open"),
                        high,
                        low,
                        close: f("close"),
                        volume: f("jdiff_vol"),
                    });
                }
            }
        }
        rows.sort_by(|a, b| a.ts.cmp(&b.ts));
        rows
    }
}
