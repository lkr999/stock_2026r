//! Stock universe loaded once from `files/search_item.csv`.
//!
//! CSV columns: 종목(name), 코드(code), 시장(market), 현재가(price),
//! 전일종가(prev_close), 등락률(change_pct), 거래량(volume).

use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One universe row (a tradeable stock with its last-known quote).
#[derive(Clone, Serialize)]
pub struct Stock {
    pub code: String,
    pub name: String,
    pub market: String,
    pub price: f64,
    pub prev_close: f64,
    pub change_pct: f64,
    pub volume: f64,
}

static UNIVERSE: OnceLock<Vec<Stock>> = OnceLock::new();
static BY_CODE: OnceLock<HashMap<String, Stock>> = OnceLock::new();
/// CSV 안의 `현재가 업데이트시각` 최댓값 — 이 스냅샷이 언제 것인지.
static AS_OF: OnceLock<String> = OnceLock::new();

fn num(s: &str) -> f64 {
    s.trim().replace(',', "").parse().unwrap_or(0.0)
}

/// Parse the CSV once; subsequent calls reuse the cached vector.
pub fn load() -> &'static Vec<Stock> {
    UNIVERSE.get_or_init(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("files/search_item.csv");
        let Ok(mut rdr) = csv::ReaderBuilder::new().from_path(&path) else {
            tracing::warn!("universe csv not found: {}", path.display());
            return vec![];
        };
        // Map header names → column indices (CSV is Korean-headed).
        let headers = rdr.headers().cloned().unwrap_or_default();
        let idx = |name: &str| headers.iter().position(|h| h == name);
        let (ci, ni, mi, pi, pci, chi, vi) = (
            idx("코드"), idx("종목"), idx("시장"), idx("현재가"),
            idx("전일종가"), idx("등락률"), idx("거래량"),
        );
        let ui = idx("현재가 업데이트시각");
        let get = |rec: &csv::StringRecord, i: Option<usize>| i.and_then(|i| rec.get(i)).unwrap_or("").to_string();
        let mut rows = vec![];
        let mut as_of = String::new();
        for rec in rdr.records().flatten() {
            let code = get(&rec, ci).trim().to_string();
            if code.is_empty() {
                continue;
            }
            let stamp = get(&rec, ui).trim().to_string();
            if stamp > as_of {
                as_of = stamp;
            }
            rows.push(Stock {
                name: get(&rec, ni).trim().to_string(),
                market: get(&rec, mi).trim().to_string(),
                price: num(&get(&rec, pi)),
                prev_close: num(&get(&rec, pci)),
                change_pct: num(&get(&rec, chi)),
                volume: num(&get(&rec, vi)),
                code,
            });
        }
        let _ = AS_OF.set(as_of);
        rows
    })
}

/// 유니버스 스냅샷 시각과 신선도. **이 CSV 는 자동 갱신되지 않는다** —
/// ①단계 가격 필터는 여기 담긴 *과거* 현재가로 후보를 거르므로, 오래되면
/// 지금 가격대와 전혀 다른 종목이 후보에 들어온다.
pub fn snapshot_info() -> (String, i64) {
    load(); // AS_OF 초기화 보장
    let as_of = AS_OF.get().cloned().unwrap_or_default();
    let age_days = chrono::DateTime::parse_from_rfc3339(&as_of)
        .map(|d| (chrono::Utc::now() - d.with_timezone(&chrono::Utc)).num_days())
        .unwrap_or(-1);
    (as_of, age_days)
}

fn by_code() -> &'static HashMap<String, Stock> {
    BY_CODE.get_or_init(|| load().iter().map(|s| (s.code.clone(), s.clone())).collect())
}

/// Filter by market and current-price / change-rate ranges.
pub fn filter(
    market: &str,
    min_price: Option<f64>,
    max_price: Option<f64>,
    min_change: Option<f64>,
    max_change: Option<f64>,
) -> Vec<Stock> {
    load()
        .iter()
        .filter(|r| market == "ALL" || market.is_empty() || r.market == market)
        .filter(|r| min_price.map_or(true, |m| r.price >= m))
        .filter(|r| max_price.map_or(true, |m| r.price <= m))
        .filter(|r| min_change.map_or(true, |m| r.change_pct >= m))
        .filter(|r| max_change.map_or(true, |m| r.change_pct <= m))
        .cloned()
        .collect()
}

/// Stock name for a code (empty string if unknown).
pub fn name_for(code: &str) -> String {
    by_code().get(code).map(|s| s.name.clone()).unwrap_or_default()
}

/// Full row for a code, if present.
pub fn info_for(code: &str) -> Option<&'static Stock> {
    by_code().get(code)
}
