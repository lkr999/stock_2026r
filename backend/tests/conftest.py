"""Test fixtures: hand-crafted candle sequences for each pattern."""

from __future__ import annotations

import pytest


def _bar(ts, o, h, l, c, v=100000.0):
    return {"ts": ts, "open": o, "high": h, "low": l, "close": c, "volume": v}


def _downtrend(n=14, start=10000.0, step=120.0):
    rows = []
    p = start
    for i in range(n):
        o = p
        c = p - step
        rows.append(_bar(f"d{i}", o, o + 20, c - 20, c))
        p = c
    return rows


def _uptrend(n=14, start=10000.0, step=120.0):
    rows = []
    p = start
    for i in range(n):
        o = p
        c = p + step
        rows.append(_bar(f"u{i}", o, c + 20, o - 20, c))
        p = c
    return rows


@pytest.fixture
def bullish_engulfing_candles():
    rows = _downtrend(14, start=10000.0)
    base = rows[-1]["close"]
    rows.append(_bar("c1", base, base + 10, base - 110, base - 100))  # small bear
    c1c = base - 100
    rows.append(_bar("c2", c1c - 30, base + 60, c1c - 40, base + 50, v=600000.0))  # engulfs
    return rows


@pytest.fixture
def bearish_engulfing_candles():
    rows = _uptrend(14, start=10000.0)
    base = rows[-1]["close"]
    rows.append(_bar("c1", base, base + 110, base - 10, base + 100))  # small bull
    c1c = base + 100
    rows.append(_bar("c2", c1c + 30, c1c + 40, base - 60, base - 50, v=600000.0))  # engulfs down
    return rows


@pytest.fixture
def hammer_candles():
    rows = _downtrend(14, start=10000.0)
    base = rows[-1]["close"]
    # hammer: long lower shadow (>=2x body), small body near top, tiny upper shadow
    o = base
    c = base + 30
    low = base - 200
    high = c + 5
    rows.append(_bar("hammer", o, high, low, c, v=300000.0))
    return rows
