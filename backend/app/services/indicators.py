"""Technical indicators (보조지표) computed from OHLCV candles.

Each function returns a list aligned to the input candles (same length), with
None during the warmup period so the frontend can plot them index-by-index.
"""

import numpy as np


def _closes(candles: list[dict]) -> np.ndarray:
    return np.array([c.get("close", 0.0) for c in candles], dtype=float)


def sma(candles: list[dict], period: int) -> list:
    closes = _closes(candles)
    out: list = [None] * len(closes)
    for i in range(period - 1, len(closes)):
        out[i] = float(closes[i - period + 1: i + 1].mean())
    return out


def _ema_array(values: np.ndarray, period: int) -> np.ndarray:
    ema = np.full(len(values), np.nan)
    if len(values) < period:
        return ema
    k = 2 / (period + 1)
    seed = values[:period].mean()
    ema[period - 1] = seed
    for i in range(period, len(values)):
        ema[i] = values[i] * k + ema[i - 1] * (1 - k)
    return ema


def ema(candles: list[dict], period: int) -> list:
    arr = _ema_array(_closes(candles), period)
    return [None if np.isnan(v) else float(v) for v in arr]


def rsi(candles: list[dict], period: int = 14) -> list:
    closes = _closes(candles)
    out: list = [None] * len(closes)
    if len(closes) <= period:
        return out
    deltas = np.diff(closes)
    gains = np.where(deltas > 0, deltas, 0.0)
    losses = np.where(deltas < 0, -deltas, 0.0)
    avg_gain = gains[:period].mean()
    avg_loss = losses[:period].mean()
    for i in range(period, len(closes)):
        g = gains[i - 1]
        l = losses[i - 1]
        avg_gain = (avg_gain * (period - 1) + g) / period
        avg_loss = (avg_loss * (period - 1) + l) / period
        rs = avg_gain / avg_loss if avg_loss > 1e-12 else float("inf")
        out[i] = 100.0 if avg_loss <= 1e-12 else float(100 - 100 / (1 + rs))
    return out


def macd(candles: list[dict], fast: int = 12, slow: int = 26, signal: int = 9) -> dict:
    closes = _closes(candles)
    ema_fast = _ema_array(closes, fast)
    ema_slow = _ema_array(closes, slow)
    macd_line = ema_fast - ema_slow
    # signal = EMA(signal) of the macd line (ignoring leading NaN)
    sig = np.full(len(closes), np.nan)
    valid = ~np.isnan(macd_line)
    if valid.any():
        start = int(np.argmax(valid))
        seg = macd_line[start:]
        seg_ema = _ema_array(seg, signal)
        sig[start:] = seg_ema
    hist = macd_line - sig

    def clean(a: np.ndarray) -> list:
        return [None if np.isnan(v) else float(v) for v in a]

    return {"macd": clean(macd_line), "signal": clean(sig), "hist": clean(hist)}


def bollinger(candles: list[dict], period: int = 20, mult: float = 2.0) -> dict:
    closes = _closes(candles)
    mid: list = [None] * len(closes)
    upper: list = [None] * len(closes)
    lower: list = [None] * len(closes)
    for i in range(period - 1, len(closes)):
        window = closes[i - period + 1: i + 1]
        m = float(window.mean())
        sd = float(window.std())
        mid[i] = m
        upper[i] = m + mult * sd
        lower[i] = m - mult * sd
    return {"mid": mid, "upper": upper, "lower": lower}


def compute_all(candles: list[dict]) -> dict:
    """Bundle the standard indicator set used by the trading chart."""
    bb = bollinger(candles, 20, 2.0)
    mac = macd(candles)
    return {
        "ma5": sma(candles, 5),
        "ma20": sma(candles, 20),
        "ma60": sma(candles, 60),
        "bb_upper": bb["upper"],
        "bb_mid": bb["mid"],
        "bb_lower": bb["lower"],
        "rsi14": rsi(candles, 14),
        "macd": mac["macd"],
        "macd_signal": mac["signal"],
        "macd_hist": mac["hist"],
    }
