//! Application settings loaded from the environment / `.env` (mirrors config.py).

use std::env;

/// Read a string env var with a fallback default.
fn s(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}
/// Read a numeric env var with a fallback default.
fn n<T: std::str::FromStr>(key: &str, default: T) -> T {
    env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
/// Read a boolean env var (`true`/`1`) with a fallback default.
fn b(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(v) => matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => default,
    }
}

/// All runtime configuration; loaded once at startup.
#[derive(Clone)]
pub struct Settings {
    // eBest credentials
    pub ebest_app_key: String,
    pub ebest_app_secret: String,
    pub ebest_url: String,
    pub ebest_verify_ssl: bool,
    // Server
    pub backend_host: String,
    pub backend_port: u16,
    pub cors_origins: String,
    // Trading defaults (spec section 10)
    pub trading_mode: String,
    pub trading_default_strategy: String,
    pub trading_max_positions: usize,
    pub trading_risk_per_trade: f64,
    pub trading_daily_loss_limit: f64,
    pub trading_paper_seed: f64,
    // Order defaults (spec section 10-2)
    pub trading_order_type: String,
    pub trading_fixed_qty: i64,
    pub trading_sell_all: bool,
    pub trading_max_buy_amount: f64,
    // Live-trading readiness advisory thresholds (spec section 11-2)
    pub live_min_paper_trades: usize,
    pub live_min_paper_days: i64,
    pub live_min_win_rate: f64,
    pub live_min_profit_factor: f64,
    pub live_require_positive_pnl: bool,
    pub live_allow_force: bool,
}

impl Settings {
    /// Build settings from process environment (after `.env` files are loaded).
    pub fn load() -> Self {
        Self {
            ebest_app_key: s("EBEST_APP_KEY", ""),
            ebest_app_secret: s("EBEST_APP_SECRET", ""),
            ebest_url: s("EBEST_URL", "https://openapi.ebestsec.co.kr"),
            ebest_verify_ssl: b("EBEST_VERIFY_SSL", true),
            backend_host: s("BACKEND_HOST", "0.0.0.0"),
            backend_port: n("BACKEND_PORT", 7001),
            cors_origins: s("CORS_ORIGINS", "http://localhost:5173"),
            trading_mode: s("TRADING_MODE", "paper"),
            trading_default_strategy: s("TRADING_DEFAULT_STRATEGY", "balanced"),
            trading_max_positions: n("TRADING_MAX_POSITIONS", 5),
            trading_risk_per_trade: n("TRADING_RISK_PER_TRADE", 0.01),
            trading_daily_loss_limit: n("TRADING_DAILY_LOSS_LIMIT", 0.03),
            trading_paper_seed: n("TRADING_PAPER_SEED", 10_000_000.0),
            trading_order_type: s("TRADING_ORDER_TYPE", "limit"),
            trading_fixed_qty: n("TRADING_FIXED_QTY", 0),
            trading_sell_all: b("TRADING_SELL_ALL", true),
            trading_max_buy_amount: n("TRADING_MAX_BUY_AMOUNT", 500_000.0),
            live_min_paper_trades: n("LIVE_MIN_PAPER_TRADES", 30),
            live_min_paper_days: n("LIVE_MIN_PAPER_DAYS", 14),
            live_min_win_rate: n("LIVE_MIN_WIN_RATE", 0.45),
            live_min_profit_factor: n("LIVE_MIN_PROFIT_FACTOR", 1.3),
            live_require_positive_pnl: b("LIVE_REQUIRE_POSITIVE_PNL", true),
            live_allow_force: b("LIVE_ALLOW_FORCE", false),
        }
    }

    /// True when both eBest API keys are present.
    pub fn ebest_configured(&self) -> bool {
        !self.ebest_app_key.is_empty() && !self.ebest_app_secret.is_empty()
    }
}
