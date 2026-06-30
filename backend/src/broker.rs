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

/// Result of a fill attempt.
pub struct Fill {
    pub ok: bool,
    pub fill_price: f64,
}

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

    /// Open a short (paper-simulation only). Live shorting is rejected because
    /// KR retail intraday short-selling is effectively unavailable.
    pub async fn sell_short(&self, _token: &str, code: &str, qty: i64, price: f64, _order_type: &str) -> Fill {
        if self.mode == TradingMode::Live {
            tracing::warn!("[LIVE] short open unsupported — rejected ({code})");
            return Fill { ok: false, fill_price: price };
        }
        if qty <= 0 {
            return Fill { ok: false, fill_price: price };
        }
        tracing::info!("[PAPER] SELL_SHORT {code} x{qty} @{price:.0}");
        Fill { ok: true, fill_price: price }
    }

    /// Cover (buy to close) a short (paper-simulation only).
    pub async fn cover(&self, _token: &str, code: &str, qty: i64, price: f64, _order_type: &str) -> Fill {
        if self.mode == TradingMode::Live {
            tracing::warn!("[LIVE] short cover unsupported — rejected ({code})");
            return Fill { ok: false, fill_price: price };
        }
        if qty <= 0 {
            return Fill { ok: false, fill_price: price };
        }
        tracing::info!("[PAPER] COVER {code} x{qty} @{price:.0}");
        Fill { ok: true, fill_price: price }
    }

    async fn order(&self, token: &str, code: &str, side: &str, qty: i64, price: f64, order_type: &str) -> Fill {
        if qty <= 0 {
            return Fill { ok: false, fill_price: price };
        }
        if self.mode == TradingMode::Paper {
            tracing::info!("[PAPER] {} {code} x{qty} @{price:.0} ({order_type})", side.to_uppercase());
            return Fill { ok: true, fill_price: price };
        }
        let Some(ebest) = &self.ebest else {
            tracing::error!("[LIVE] no eBest connection — order skipped");
            return Fill { ok: false, fill_price: price };
        };
        let res = ebest.place_order(token, code, side, qty, price, resolve_price_type(order_type)).await;
        let ok = res.get("rsp_cd").and_then(Value::as_str) == Some("0000");
        tracing::info!("[LIVE] {} {code} x{qty} @{price:.0} -> {:?}", side.to_uppercase(), res.get("rsp_cd"));
        Fill { ok, fill_price: price }
    }
}
