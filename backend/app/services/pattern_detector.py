"""Caginalp & Laurent (1998) 8-pattern detector + modern overlays (sections 4, 9).

All eight reversal patterns are rule-based and deterministic. ATR-adaptive
thresholds and volume confirmation (section 9) are applied as overlays, and the
composite score is computed via StrategyConfig (section 10-1), not hardcoded.
"""

from dataclasses import asdict, dataclass
from typing import Any

import numpy as np

from app.services.strategy import STRATEGY_PRESETS, SignalSource, StrategyConfig
from app.services.timeframe import TIMEFRAME_CONFIGS, Timeframe

type Raw = dict[str, Any]


@dataclass
class Candle:
    ts: str
    open: float
    high: float
    low: float
    close: float
    volume: float = 0.0

    @property
    def body(self) -> float:
        return abs(self.close - self.open)

    @property
    def range(self) -> float:
        return (self.high - self.low) or 1e-9

    @property
    def body_ratio(self) -> float:
        return self.body / self.range

    @property
    def upper_shadow(self) -> float:
        return self.high - max(self.open, self.close)

    @property
    def lower_shadow(self) -> float:
        return min(self.open, self.close) - self.low

    @property
    def is_bull(self) -> bool:
        return self.close >= self.open

    @property
    def is_bear(self) -> bool:
        return self.close < self.open


@dataclass
class PatternResult:
    pattern_name: str
    pattern_type: str  # "bullish" | "bearish"
    detected_at: str
    confidence: float
    candles_used: list[Raw]
    expected_5: float
    expected_10: float
    expected_25: float
    atr_normalized: bool = False
    volume_confirmed: bool = False
    mtf_score: float = 0.0
    ml_score: float = 0.0
    gaf_score: float = 0.0
    composite_score: float = 0.0

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


# Caginalp & Laurent (1998) Table 3 — 5/10/25-bar excess returns (%).
PAPER_RETURNS: dict[str, tuple[float, float, float]] = {
    "three_white_soldiers": (1.78, 2.43, 3.21),
    "morning_star": (1.45, 1.89, 2.67),
    "hammer": (0.89, 1.34, 2.11),
    "bullish_engulfing": (1.12, 1.56, 2.34),
    "three_black_crows": (-1.65, -2.28, -3.10),
    "evening_star": (-1.38, -1.82, -2.55),
    "hanging_man": (-0.76, -1.21, -1.98),
    "bearish_engulfing": (-1.05, -1.49, -2.22),
}


def _trend_slope(candles: list[Candle]) -> float:
    if len(candles) < 2:
        return 0.0
    closes = np.array([c.close for c in candles], dtype=float)
    x = np.arange(len(closes))
    slope = np.polyfit(x, closes, 1)[0]
    return float(slope)


def _avg_body(candles: list[Candle]) -> float:
    return float(np.mean([c.body for c in candles])) or 1e-9


def _result(name: str, ptype: str, ts: str, conf: float, used: list[Candle]) -> PatternResult:
    e5, e10, e25 = PAPER_RETURNS[name]
    return PatternResult(
        pattern_name=name,
        pattern_type=ptype,
        detected_at=ts,
        confidence=min(1.0, max(0.0, conf)),
        candles_used=[asdict(c) for c in used],
        expected_5=e5,
        expected_10=e10,
        expected_25=e25,
    )


# --------------------------------------------------------------------------
# eight pattern detectors
# --------------------------------------------------------------------------

def detect_three_white_soldiers(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    if _trend_slope(candles[-(lookback + 3):-3]) >= 0:
        return None
    c1, c2, c3 = candles[-3], candles[-2], candles[-1]
    if not (c1.is_bull and c2.is_bull and c3.is_bull):
        return None
    if not (c1.open < c2.open < max(c1.open, c1.close)):
        return None
    if not (c2.open < c3.open < max(c2.open, c2.close)):
        return None
    if not (c1.close < c2.close < c3.close):
        return None
    body_ratios = [c1.body_ratio, c2.body_ratio, c3.body_ratio]
    if min(body_ratios) < 0.60:
        return None
    conf = min(body_ratios) / 0.60 * 0.5 + (c3.close - c1.open) / _avg_body([c1, c2, c3]) / 3 * 0.5
    return _result("three_white_soldiers", "bullish", c3.ts, conf, [c1, c2, c3])


def detect_three_black_crows(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    if _trend_slope(candles[-(lookback + 3):-3]) <= 0:
        return None
    c1, c2, c3 = candles[-3], candles[-2], candles[-1]
    if not (c1.is_bear and c2.is_bear and c3.is_bear):
        return None
    if not (c1.close > c2.close > c3.close):
        return None
    if not (c2.open < c1.open and c3.open < c2.open):
        return None
    body_ratios = [c1.body_ratio, c2.body_ratio, c3.body_ratio]
    if min(body_ratios) < 0.60:
        return None
    conf = min(body_ratios) / 0.60 * 0.6 + 0.4
    return _result("three_black_crows", "bearish", c3.ts, conf, [c1, c2, c3])


def detect_morning_star(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    if _trend_slope(candles[-(lookback + 3):-3]) >= 0:
        return None
    c1, c2, c3 = candles[-3], candles[-2], candles[-1]
    if not c1.is_bear or c1.body_ratio < 0.70:
        return None
    if max(c2.open, c2.close) >= min(c1.open, c1.close):
        return None
    if not c3.is_bull:
        return None
    recovery = (c3.close - c2.close) / (c1.body or 1e-9)
    if recovery < 0.50:
        return None
    conf = recovery * 0.6 + c1.body_ratio * 0.4
    return _result("morning_star", "bullish", c3.ts, conf, [c1, c2, c3])


def detect_evening_star(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    if _trend_slope(candles[-(lookback + 3):-3]) <= 0:
        return None
    c1, c2, c3 = candles[-3], candles[-2], candles[-1]
    if not c1.is_bull or c1.body_ratio < 0.70:
        return None
    if min(c2.open, c2.close) <= max(c1.open, c1.close):
        return None
    if not c3.is_bear:
        return None
    decline = (c2.open - c3.close) / (c1.body or 1e-9)
    if decline < 0.50:
        return None
    conf = decline * 0.6 + c1.body_ratio * 0.4
    return _result("evening_star", "bearish", c3.ts, conf, [c1, c2, c3])


def _is_hammer_shape(c: Candle) -> tuple[bool, float]:
    if c.range < 1e-6:
        return False, 0.0
    if c.lower_shadow < c.body * 2.0:
        return False, 0.0
    if c.upper_shadow > c.body * 0.3:
        return False, 0.0
    body_pos = (min(c.open, c.close) - c.low) / c.range
    if body_pos < 0.60:
        return False, 0.0
    conf = (c.lower_shadow / (c.body * 2.0 or 1e-9)) * 0.7 + body_pos * 0.3
    return True, min(1.0, conf)


def detect_hammer(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 1:
        return None
    if _trend_slope(candles[-(lookback + 1):-1]) >= 0:
        return None
    c = candles[-1]
    valid, conf = _is_hammer_shape(c)
    if not valid:
        return None
    return _result("hammer", "bullish", c.ts, conf, [c])


def detect_hanging_man(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 1:
        return None
    if _trend_slope(candles[-(lookback + 1):-1]) <= 0:
        return None
    c = candles[-1]
    valid, conf = _is_hammer_shape(c)
    if not valid:
        return None
    return _result("hanging_man", "bearish", c.ts, conf, [c])


def detect_bullish_engulfing(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 2:
        return None
    if _trend_slope(candles[-(lookback + 2):-2]) >= 0:
        return None
    c1, c2 = candles[-2], candles[-1]
    if not c1.is_bear or not c2.is_bull:
        return None
    if not (c2.open < c1.close and c2.close > c1.open):
        return None
    engulf_ratio = c2.body / (c1.body or 1e-9)
    conf = (engulf_ratio - 1.0) * 0.5 + 0.5
    return _result("bullish_engulfing", "bullish", c2.ts, conf, [c1, c2])


def detect_bearish_engulfing(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 2:
        return None
    if _trend_slope(candles[-(lookback + 2):-2]) <= 0:
        return None
    c1, c2 = candles[-2], candles[-1]
    if not c1.is_bull or not c2.is_bear:
        return None
    if not (c2.open > c1.close and c2.close < c1.open):
        return None
    engulf_ratio = c2.body / (c1.body or 1e-9)
    conf = (engulf_ratio - 1.0) * 0.5 + 0.5
    return _result("bearish_engulfing", "bearish", c2.ts, conf, [c1, c2])


DETECTORS = [
    detect_three_white_soldiers,
    detect_morning_star,
    detect_hammer,
    detect_bullish_engulfing,
    detect_three_black_crows,
    detect_evening_star,
    detect_hanging_man,
    detect_bearish_engulfing,
]


# --------------------------------------------------------------------------
# modern overlays (section 9)
# --------------------------------------------------------------------------

def compute_atr(candles: list[Candle], period: int = 14) -> float:
    if len(candles) < period + 1:
        return 0.0
    trs = []
    for i in range(1, period + 1):
        c = candles[-i]
        p = candles[-(i + 1)]
        trs.append(max(c.high - c.low, abs(c.high - p.close), abs(c.low - p.close)))
    return float(np.mean(trs)) or 1e-9


def _check_atr_threshold(candles: list[Candle], atr: float, min_body_atr_ratio: float = 0.3) -> bool:
    if not candles or atr <= 0:
        return False
    return candles[-1].body >= atr * min_body_atr_ratio


def _check_volume_spike(candles: list[Candle], period: int = 20, threshold: float = 1.5) -> bool:
    if len(candles) < period + 1:
        return False
    recent = [c.volume for c in candles[-(period + 1):-1]]
    avg_vol = float(np.mean(recent)) or 1e-9
    if avg_vol <= 1e-9:
        return False
    return candles[-1].volume >= avg_vol * threshold


def apply_strategy(result: PatternResult, cfg: StrategyConfig) -> None:
    signals = {
        SignalSource.RULE: result.confidence,
        SignalSource.ATR: 1.0 if result.atr_normalized else 0.0,
        SignalSource.VOLUME: 1.0 if result.volume_confirmed else 0.0,
        SignalSource.MTF: result.mtf_score,
        SignalSource.ML: result.ml_score,
        SignalSource.GAF: result.gaf_score,
    }
    result.composite_score = cfg.composite(signals)


def to_candles(raw_candles: list[Raw]) -> list[Candle]:
    fields = Candle.__dataclass_fields__
    return [Candle(**{k: v for k, v in r.items() if k in fields}) for r in raw_candles]


class PatternDetector:
    def scan(
        self,
        raw_candles: list[Raw],
        tf: Timeframe,
        min_confidence: float = 0.0,
        use_modern: bool = True,
        ml_model=None,
        strategy: StrategyConfig | None = None,
        recent_bars: int = 1,
    ) -> list[PatternResult]:
        """최근 봉에서 패턴을 탐지한다.

        각 패턴 탐지기는 시계열의 '마지막 봉'을 기준으로만 동작하므로, 마지막 봉
        하나만 보면 패턴이 거의 잡히지 않는다(스캔 결과가 비는 주된 원인).
        ``recent_bars`` 만큼 끝 지점을 뒤로 밀어가며 탐지해 최근 N봉 내에 형성된
        패턴까지 함께 찾는다. 기본값 1은 종전 동작(마지막 봉만)과 동일해 실시간
        매매 엔진에는 영향이 없다.
        """
        cfg = TIMEFRAME_CONFIGS[tf]
        candles = to_candles(raw_candles)
        if len(candles) < 3:
            return []

        strategy = strategy or STRATEGY_PRESETS["balanced"]
        vol_period = 10 if tf != Timeframe.D1 else 20

        seen: set[tuple[str, str]] = set()
        results = []
        for offset in range(max(1, recent_bars)):
            end = len(candles) - offset
            if end < 3:
                break
            window = candles[:end]
            atr = compute_atr(window) if use_modern else 0.0
            for detector in DETECTORS:
                result = detector(window, lookback=cfg.trend_lookback)
                if not result:
                    continue
                key = (result.pattern_name, result.detected_at)
                if key in seen:
                    continue
                if use_modern:
                    result.atr_normalized = _check_atr_threshold(window, atr)
                    result.volume_confirmed = _check_volume_spike(window, period=vol_period)
                    if ml_model is not None:
                        try:
                            from app.services.ml_features import extract_ml_features

                            feats = extract_ml_features(window, cfg.trend_lookback)
                            result.ml_score = float(ml_model.predict_proba([feats])[0][1])
                        except Exception:
                            result.ml_score = 0.0
                    apply_strategy(result, strategy)
                else:
                    result.composite_score = result.confidence
                if result.confidence >= min_confidence:
                    seen.add(key)
                    results.append(result)

        results.sort(key=lambda r: r.composite_score, reverse=True)
        return results
