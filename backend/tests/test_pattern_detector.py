from __future__ import annotations

from app.services.pattern_detector import PatternDetector, compute_atr, to_candles
from app.services.timeframe import Timeframe


def test_detects_bullish_engulfing(bullish_engulfing_candles):
    det = PatternDetector()
    results = det.scan(bullish_engulfing_candles, Timeframe.D1, use_modern=False)
    names = [r.pattern_name for r in results]
    assert "bullish_engulfing" in names


def test_detects_bearish_engulfing(bearish_engulfing_candles):
    det = PatternDetector()
    results = det.scan(bearish_engulfing_candles, Timeframe.D1, use_modern=False)
    names = [r.pattern_name for r in results]
    assert "bearish_engulfing" in names


def test_detects_hammer(hammer_candles):
    det = PatternDetector()
    results = det.scan(hammer_candles, Timeframe.D1, use_modern=False)
    names = [r.pattern_name for r in results]
    assert "hammer" in names


def test_confidence_is_clamped(bullish_engulfing_candles):
    det = PatternDetector()
    results = det.scan(bullish_engulfing_candles, Timeframe.D1, use_modern=False)
    for r in results:
        assert 0.0 <= r.confidence <= 1.0


def test_atr_positive(bullish_engulfing_candles):
    atr = compute_atr(to_candles(bullish_engulfing_candles))
    assert atr > 0


def test_empty_candles_no_crash():
    det = PatternDetector()
    assert det.scan([], Timeframe.D1) == []


def test_strategy_composite_excludes_inactive(bullish_engulfing_candles):
    from app.services.strategy import STRATEGY_PRESETS

    det = PatternDetector()
    results = det.scan(bullish_engulfing_candles, Timeframe.D1, strategy=STRATEGY_PRESETS["aggressive"])
    # aggressive has MTF=0, ML=0 -> composite must stay within [0,1]
    for r in results:
        assert 0.0 <= r.composite_score <= 1.0
