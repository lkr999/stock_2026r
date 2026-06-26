//! HTTP routers, one module per resource group. Each `router()` returns absolute
//! `/api/...` routes that `main` merges into the app.

pub mod backtest;
pub mod candles;
pub mod misc;
pub mod patterns;
pub mod signals;
pub mod trading;
