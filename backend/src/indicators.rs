//! Technical indicators computed from OHLCV candles (보조지표).
//!
//! Each series is aligned to the input candles (same length) with `None` during
//! the warmup period so the frontend can plot them index-by-index.

use crate::candle::Candle;
use serde::Serialize;

type Series = Vec<Option<f64>>;

fn closes(candles: &[Candle]) -> Vec<f64> {
    candles.iter().map(|c| c.close).collect()
}

/// Simple moving average over `period` closes.
pub fn sma(candles: &[Candle], period: usize) -> Series {
    let c = closes(candles);
    let mut out = vec![None; c.len()];
    if period == 0 {
        return out;
    }
    for i in (period - 1)..c.len() {
        let mean = c[i + 1 - period..=i].iter().sum::<f64>() / period as f64;
        out[i] = Some(mean);
    }
    out
}

/// EMA array over raw values, NaN during warmup (seeded with an SMA).
fn ema_array(values: &[f64], period: usize) -> Vec<f64> {
    let mut ema = vec![f64::NAN; values.len()];
    if values.len() < period || period == 0 {
        return ema;
    }
    let k = 2.0 / (period as f64 + 1.0);
    let seed = values[..period].iter().sum::<f64>() / period as f64;
    ema[period - 1] = seed;
    for i in period..values.len() {
        ema[i] = values[i] * k + ema[i - 1] * (1.0 - k);
    }
    ema
}

fn clean(a: &[f64]) -> Series {
    a.iter().map(|&v| if v.is_nan() { None } else { Some(v) }).collect()
}

/// Wilder-smoothed RSI over `period` closes.
pub fn rsi(candles: &[Candle], period: usize) -> Series {
    let c = closes(candles);
    let mut out = vec![None; c.len()];
    if c.len() <= period {
        return out;
    }
    let deltas: Vec<f64> = c.windows(2).map(|w| w[1] - w[0]).collect();
    let gains: Vec<f64> = deltas.iter().map(|&d| if d > 0.0 { d } else { 0.0 }).collect();
    let losses: Vec<f64> = deltas.iter().map(|&d| if d < 0.0 { -d } else { 0.0 }).collect();
    let mut avg_gain = gains[..period].iter().sum::<f64>() / period as f64;
    let mut avg_loss = losses[..period].iter().sum::<f64>() / period as f64;
    for i in period..c.len() {
        avg_gain = (avg_gain * (period as f64 - 1.0) + gains[i - 1]) / period as f64;
        avg_loss = (avg_loss * (period as f64 - 1.0) + losses[i - 1]) / period as f64;
        out[i] = Some(if avg_loss <= 1e-12 {
            100.0
        } else {
            let rs = avg_gain / avg_loss;
            100.0 - 100.0 / (1.0 + rs)
        });
    }
    out
}

/// MACD line / signal / histogram (12-26-9 by default).
pub fn macd(candles: &[Candle], fast: usize, slow: usize, signal: usize) -> (Series, Series, Series) {
    let c = closes(candles);
    let ema_fast = ema_array(&c, fast);
    let ema_slow = ema_array(&c, slow);
    let macd_line: Vec<f64> = ema_fast.iter().zip(&ema_slow).map(|(f, s)| f - s).collect();
    // signal = EMA(signal) of the macd line, skipping leading NaN.
    let mut sig = vec![f64::NAN; c.len()];
    if let Some(start) = macd_line.iter().position(|v| !v.is_nan()) {
        let seg_ema = ema_array(&macd_line[start..], signal);
        for (i, v) in seg_ema.into_iter().enumerate() {
            sig[start + i] = v;
        }
    }
    let hist: Vec<f64> = macd_line.iter().zip(&sig).map(|(m, s)| m - s).collect();
    (clean(&macd_line), clean(&sig), clean(&hist))
}

/// Bollinger bands (mid / upper / lower) over `period` closes.
pub fn bollinger(candles: &[Candle], period: usize, mult: f64) -> (Series, Series, Series) {
    let c = closes(candles);
    let mut mid = vec![None; c.len()];
    let mut upper = vec![None; c.len()];
    let mut lower = vec![None; c.len()];
    if period == 0 {
        return (mid, upper, lower);
    }
    for i in (period - 1)..c.len() {
        let w = &c[i + 1 - period..=i];
        let m = w.iter().sum::<f64>() / period as f64;
        let var = w.iter().map(|x| (x - m).powi(2)).sum::<f64>() / period as f64;
        let sd = var.sqrt();
        mid[i] = Some(m);
        upper[i] = Some(m + mult * sd);
        lower[i] = Some(m - mult * sd);
    }
    (mid, upper, lower)
}

/// Standard indicator bundle used by the trading chart.
#[derive(Serialize)]
pub struct Indicators {
    pub ma5: Series,
    pub ma20: Series,
    pub ma60: Series,
    pub bb_upper: Series,
    pub bb_mid: Series,
    pub bb_lower: Series,
    pub rsi14: Series,
    pub macd: Series,
    pub macd_signal: Series,
    pub macd_hist: Series,
}

/// Compute every indicator the frontend chart needs in one pass.
pub fn compute_all(candles: &[Candle]) -> Indicators {
    let (bb_mid, bb_upper, bb_lower) = bollinger(candles, 20, 2.0);
    let (macd_line, macd_signal, macd_hist) = macd(candles, 12, 26, 9);
    Indicators {
        ma5: sma(candles, 5),
        ma20: sma(candles, 20),
        ma60: sma(candles, 60),
        bb_upper,
        bb_mid,
        bb_lower,
        rsi14: rsi(candles, 14),
        macd: macd_line,
        macd_signal,
        macd_hist,
    }
}
