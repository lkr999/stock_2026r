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
    /// 백테스트용 심층 조회(연속조회로 합친 긴 히스토리) 전용 캐시.
    /// 실시간 엔진 캐시와 분리해 서로 TTL/크기에 영향을 주지 않는다.
    /// 값 = (만료시각, 이 항목을 채울 때 *요청했던* 봉수, 캔들).
    hist_cache: Mutex<HashMap<(String, Timeframe), (Instant, usize, Vec<Candle>)>>,
}

/// 심층 조회 페이지 수 상한 — eBest 호출당 최대 500봉이므로 4페이지 ≈ 2000봉.
const HIST_MAX_PAGES: usize = 4;
/// 심층 조회 캐시 TTL — 백테스트 반복 실행 시 재조회를 막는다 (5분).
const HIST_TTL: Duration = Duration::from_secs(300);

impl CandleFetcher {
    pub fn new(ebest: Arc<EBestService>) -> Self {
        Self { ebest, cache: Mutex::new(HashMap::new()), hist_cache: Mutex::new(HashMap::new()) }
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
        // 조회 실패(빈 응답)는 캐시하지 않는다. 캐시에 넣으면 TTL 동안(D1 은 10분)
        // 그 종목이 계속 "데이터 부족" 으로 스킵되고, 보유 종목이면 스탑 감시까지
        // 멈춘다. 다음 사이클에 곧바로 재조회하게 둔다.
        if !rows.is_empty() {
            let mut cache = self.cache.lock().await;
            cache.insert(key, (Instant::now() + Self::cache_ttl(tf), rows.clone()));
        }
        rows
    }

    /// 백테스트용 심층 히스토리: 연속조회(cts)로 `target_bars` 에 도달할 때까지
    /// 과거 페이지를 이어 붙인다. 실시간 `fetch()` 는 건드리지 않는다 — 엔진의
    /// 사이클 예산(호출당 ~1.1초)에 영향을 주지 않기 위해 백테스트 라우트만 쓴다.
    pub async fn fetch_history(&self, token: &str, shcode: &str, tf: Timeframe, target_bars: usize) -> Vec<Candle> {
        let key = (shcode.to_string(), tf);
        {
            let cache = self.hist_cache.lock().await;
            if let Some((expiry, cached_target, rows)) = cache.get(&key) {
                // 이전에 *같거나 더 큰* 목표로 받아둔 것만 재사용한다. 사용자가
                // 심층 조회봉수를 올렸는데 짧은 캐시를 돌려주면 설정이 먹지 않는다.
                if Instant::now() < *expiry && *cached_target >= target_bars {
                    return rows.clone();
                }
            }
        }
        let rows = if tf == Timeframe::D1 {
            // t8451 은 qrycnt 로 기간을 직접 늘릴 수 있다 (sdate 를 그만큼 과거로).
            let raw = self.ebest.fetch_daily_candles(token, shcode, target_bars as i32).await;
            raw.iter().map(Self::daily_row).collect()
        } else {
            let ncnt = tf.config().ncnt;
            let mut merged: HashMap<String, Candle> = HashMap::new();
            let mut cts: Option<(String, String)> = None;
            for _ in 0..HIST_MAX_PAGES {
                let key2 = cts.as_ref().map(|(d, t)| (d.as_str(), t.as_str())).unwrap_or(("", ""));
                let raw = self.ebest.fetch_minute_candles_cts(token, shcode, ncnt, 500, key2).await;
                let page = Self::normalize_minute(&raw);
                if page.is_empty() {
                    break;
                }
                for c in page {
                    merged.insert(c.ts.clone(), c);
                }
                if merged.len() >= target_bars {
                    break;
                }
                match EBestService::minute_cts_key(&raw) {
                    Some(k) => cts = Some(k),
                    None => break, // 더 과거 데이터 없음
                }
            }
            let mut rows: Vec<Candle> = merged.into_values().collect();
            rows.sort_by(|a, b| a.ts.cmp(&b.ts));
            // 목표보다 넉넉히 모였으면 최신 target_bars 만 유지 (오래된 꼬리 절단).
            if rows.len() > target_bars {
                let cut = rows.len() - target_bars;
                rows.drain(..cut);
            }
            rows
        };
        // 실시간 캐시와 같은 이유로 빈 결과는 캐시하지 않는다 (5분간 백테스트 불가).
        if !rows.is_empty() {
            let mut cache = self.hist_cache.lock().await;
            cache.insert(key, (Instant::now() + HIST_TTL, target_bars, rows.clone()));
        }
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
