//! Timeframe definitions + per-timeframe configuration (spec section 2).

use serde::{Deserialize, Serialize};

/// Supported candle timeframes (1m … 1d).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Timeframe {
    M1,
    M3,
    M5,
    M10,
    M15,
    M30,
    H1,
    D1,
}

impl Timeframe {
    /// Wire value used by the API / frontend (e.g. `"5m"`, `"1d"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Timeframe::M1 => "1m",
            Timeframe::M3 => "3m",
            Timeframe::M5 => "5m",
            Timeframe::M10 => "10m",
            Timeframe::M15 => "15m",
            Timeframe::M30 => "30m",
            Timeframe::H1 => "60m",
            Timeframe::D1 => "1d",
        }
    }

    /// Parse a wire value; `None` for unknown strings (→ HTTP 400 upstream).
    pub fn parse(v: &str) -> Option<Self> {
        Some(match v {
            "1m" => Timeframe::M1,
            "3m" => Timeframe::M3,
            "5m" => Timeframe::M5,
            "10m" => Timeframe::M10,
            "15m" => Timeframe::M15,
            "30m" => Timeframe::M30,
            "60m" => Timeframe::H1,
            "1d" => Timeframe::D1,
            _ => return None,
        })
    }

    /// Per-timeframe fetch + trend config. (Daily uses t8451; the rest t8452.)
    pub fn config(self) -> TimeframeConfig {
        // (ncnt, qrycnt, trend_lookback)
        let (ncnt, qrycnt, lookback) = match self {
            Timeframe::M1 => (1, 500, 20),
            Timeframe::M3 => (3, 500, 15),
            Timeframe::M5 => (5, 300, 12),
            Timeframe::M10 => (10, 200, 10),
            Timeframe::M15 => (15, 200, 10),
            Timeframe::M30 => (30, 100, 8),
            Timeframe::H1 => (60, 100, 8),
            Timeframe::D1 => (0, 60, 5),
        };
        TimeframeConfig { ncnt, qrycnt, trend_lookback: lookback }
    }

    /// Higher timeframes used for MTF confluence (spec section 9-3).
    pub fn mtf_group(self) -> Vec<Timeframe> {
        use Timeframe::*;
        match self {
            M1 => vec![M5, M15],
            M3 => vec![M15, H1],
            M5 => vec![M15, H1],
            M10 => vec![M30, H1],
            M15 => vec![H1, D1],
            M30 => vec![H1, D1],
            H1 => vec![D1],
            D1 => vec![],
        }
    }
}

/// Static configuration for a single timeframe.
pub struct TimeframeConfig {
    pub ncnt: i32,
    pub qrycnt: i32,
    pub trend_lookback: usize,
}
