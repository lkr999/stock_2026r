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

/// Whether short-selling is executable at all, in *any* mode — the single
/// switch every short-related gate reads (live orders, paper entries, and the
/// backtester/OOS selection that ranks strategies before they ever trade).
/// KR retail intraday short-selling is effectively unavailable, so this is
/// `false` today; flip it here if the broker ever gains real support, and
/// every gate that reads it (`Broker::supports_short`, `backtest.rs`)
/// picks up the change automatically.
pub fn shorting_supported() -> bool {
    false
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

    /// Whether *new* short entries are executable. Delegates to the module-level
    /// `shorting_supported()` so paper mirrors live instead of simulating a
    /// trade nothing can actually place — the paper track record then only
    /// reflects strategies live can execute.
    pub fn supports_short(&self) -> bool {
        shorting_supported()
    }

    /// Open a short. Rejected in every mode — see `supports_short`.
    pub async fn sell_short(&self, _token: &str, code: &str, qty: i64, price: f64, _order_type: &str) -> Fill {
        if !self.supports_short() {
            tracing::warn!("[{}] short open unsupported — rejected ({code})", self.mode.as_str());
            return Fill { ok: false, fill_price: price };
        }
        if qty <= 0 {
            return Fill { ok: false, fill_price: price };
        }
        tracing::info!("[PAPER] SELL_SHORT {code} x{qty} @{price:.0}");
        Fill { ok: true, fill_price: price }
    }

    /// Cover (buy to close) a short. Unlike `sell_short`, this is *not* gated
    /// by `supports_short` — a short position already open (e.g. from before
    /// this policy took effect) must always be closeable, in paper the same
    /// as live. Only opening new shorts is blocked.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_short_entries_are_rejected_in_every_mode() {
        // Paper must now mirror live: neither can open a new short (only
        // close an existing one via `cover`), so the paper track record only
        // reflects strategies that are actually executable live.
        for mode in [TradingMode::Paper, TradingMode::Live] {
            let broker = Broker::new(None, mode);
            assert!(!broker.supports_short());
            let fill = broker.sell_short("", "005930", 10, 70_000.0, "limit").await;
            assert!(!fill.ok, "sell_short should be rejected in {}", mode.as_str());
        }
    }
}
