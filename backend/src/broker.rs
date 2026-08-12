//! Order execution layer (spec section 10-2).
//!
//! Paper and live mode differ *only* here: signal → risk gate → sizing is shared,
//! and only `buy`/`sell` diverge.
//! - PAPER: fills instantly at the current price (simulation; no eBest call).
//! - LIVE : sends a real eBest order (CSPAT00601).

use crate::ebest::EBestService;
use serde_json::Value;
use std::sync::Arc;

/// Paper (simulated) vs live (real eBest orders).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TradingMode {
    Paper,
    Live,
}

impl TradingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TradingMode::Paper => "paper",
            TradingMode::Live => "live",
        }
    }
    pub fn parse(s: &str) -> Self {
        if s == "live" { TradingMode::Live } else { TradingMode::Paper }
    }
}

/// Map an order type to eBest's OrdprcPtnCode (호가유형코드).
fn resolve_price_type(order_type: &str) -> &'static str {
    match order_type {
        "market" => "03", // 시장가
        "best" => "06",   // 최유리지정가
        _ => "00",        // 지정가 (limit)
    }
}

/// Result of a fill attempt. `qty` is the quantity *actually filled* — for
/// live limit orders this can be less than requested (partial fill after the
/// unfilled remainder is cancelled); 0 with `ok=false` means nothing filled.
pub struct Fill {
    pub ok: bool,
    pub fill_price: f64,
    pub qty: i64,
}

/// Live limit-order fill reconciliation: how many times to poll t0425 and how
/// long between polls before cancelling the unfilled remainder (~15s total).
const LIMIT_FILL_POLLS: u32 = 5;
const LIMIT_FILL_POLL_SECS: u64 = 3;

/// Routes orders to simulation (paper) or the eBest API (live).
pub struct Broker {
    ebest: Option<Arc<EBestService>>,
    pub mode: TradingMode,
}

impl Broker {
    pub fn new(ebest: Option<Arc<EBestService>>, mode: TradingMode) -> Self {
        Self { ebest, mode }
    }

    pub async fn buy(&self, token: &str, code: &str, qty: i64, price: f64, order_type: &str) -> Fill {
        self.order(token, code, "buy", qty, price, order_type).await
    }

    pub async fn sell(&self, token: &str, code: &str, qty: i64, price: f64, order_type: &str) -> Fill {
        self.order(token, code, "sell", qty, price, order_type).await
    }

    async fn order(&self, token: &str, code: &str, side: &str, qty: i64, price: f64, order_type: &str) -> Fill {
        if qty <= 0 {
            return Fill { ok: false, fill_price: price, qty: 0 };
        }
        if self.mode == TradingMode::Paper {
            tracing::info!("[PAPER] {} {code} x{qty} @{price:.0} ({order_type})", side.to_uppercase());
            return Fill { ok: true, fill_price: price, qty };
        }
        let Some(ebest) = &self.ebest else {
            tracing::error!("[LIVE] no eBest connection — order skipped");
            return Fill { ok: false, fill_price: price, qty: 0 };
        };
        let res = ebest.place_order(token, code, side, qty, price, resolve_price_type(order_type)).await;
        let ok = res.get("rsp_cd").and_then(Value::as_str) == Some("0000");
        tracing::info!("[LIVE] {} {code} x{qty} @{price:.0} -> {:?}", side.to_uppercase(), res.get("rsp_cd"));
        if !ok {
            return Fill { ok: false, fill_price: price, qty: 0 };
        }
        // 시장가는 즉시 체결로 간주한다 (요청가 기준; 이후 reconcile_live 가 평균단가 보정).
        if order_type == "market" {
            return Fill { ok: true, fill_price: price, qty };
        }
        // 지정가/최유리: 접수 ≠ 체결. 주문번호로 t0425 를 폴링해 실제 체결수량·
        // 평균체결가를 확인하고, 시간 내 다 안 되면 잔량을 취소한 뒤 체결분만 반환한다.
        let ordno = res
            .get("CSPAT00601OutBlock2")
            .and_then(|b| b.get("OrdNo"))
            .and_then(crate::ebest::parse_float)
            .map(|v| v as i64);
        let Some(ordno) = ordno else {
            // 주문번호를 못 받으면 대사가 불가능 — 과거처럼 전량 체결로 간주하되 경고.
            tracing::warn!("[LIVE] {code}: 주문번호 없음 — 체결 대사 생략(전량 체결 간주)");
            return Fill { ok: true, fill_price: price, qty };
        };
        let mut last: Option<(i64, f64, i64)> = None;
        for _ in 0..LIMIT_FILL_POLLS {
            tokio::time::sleep(std::time::Duration::from_secs(LIMIT_FILL_POLL_SECS)).await;
            if let Some((che_qty, che_price, rem)) = ebest.order_fill_status(token, code, ordno).await {
                last = Some((che_qty, che_price, rem));
                if rem <= 0 && che_qty > 0 {
                    let fp = if che_price > 0.0 { che_price } else { price };
                    tracing::info!("[LIVE] {code} 주문 {ordno} 전량 체결 x{che_qty} @{fp:.0}");
                    return Fill { ok: true, fill_price: fp, qty: che_qty };
                }
            }
        }
        // 타임아웃 — 미체결 잔량 취소 후 마지막 확인.
        let rem = last.map(|(_, _, r)| r).unwrap_or(qty);
        if rem > 0 {
            ebest.cancel_order(token, ordno, code, rem).await;
            if let Some(st2) = ebest.order_fill_status(token, code, ordno).await {
                last = Some(st2);
            }
        }
        match last {
            Some((che_qty, che_price, _)) if che_qty > 0 => {
                let fp = if che_price > 0.0 { che_price } else { price };
                tracing::warn!("[LIVE] {code} 주문 {ordno} 부분 체결 x{che_qty}/{qty} @{fp:.0} — 잔량 취소");
                Fill { ok: true, fill_price: fp, qty: che_qty }
            }
            _ => {
                tracing::warn!("[LIVE] {code} 주문 {ordno} 미체결 — 취소됨");
                Fill { ok: false, fill_price: price, qty: 0 }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn paper_buy_and_sell_fill_instantly() {
        let broker = Broker::new(None, TradingMode::Paper);
        let buy = broker.buy("", "005930", 10, 70_000.0, "limit").await;
        assert!(buy.ok && buy.qty == 10);
        let sell = broker.sell("", "005930", 10, 71_000.0, "limit").await;
        assert!(sell.ok && sell.qty == 10);
    }

    #[tokio::test]
    async fn non_positive_qty_is_rejected() {
        let broker = Broker::new(None, TradingMode::Paper);
        assert!(!broker.buy("", "005930", 0, 70_000.0, "limit").await.ok);
    }
}
