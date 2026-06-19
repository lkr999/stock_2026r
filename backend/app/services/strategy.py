"""Strategy mixing/selection config (spec section 10-1).

Replaces hardcoded composite weights with a user-tunable StrategyConfig so the
six signal sources (rule / atr / volume / mtf / ml / gaf) can be turned on/off
and weighted independently.
"""

from dataclasses import dataclass, field
from enum import Enum


class SignalSource(str, Enum):
    RULE = "rule"
    ATR = "atr"
    VOLUME = "volume"
    MTF = "mtf"
    ML = "ml"
    GAF = "gaf"


def _default_weights() -> dict:
    return {
        SignalSource.RULE: 0.40,
        SignalSource.ATR: 0.15,
        SignalSource.VOLUME: 0.15,
        SignalSource.MTF: 0.30,
        SignalSource.ML: 0.00,
        SignalSource.GAF: 0.00,
    }


def _default_patterns() -> list:
    return [
        "three_white_soldiers",
        "morning_star",
        "bullish_engulfing",
        "hammer",
    ]


@dataclass
class StrategyConfig:
    name: str = "balanced"
    weights: dict = field(default_factory=_default_weights)
    enabled_patterns: list = field(default_factory=_default_patterns)
    entry_threshold: float = 0.65
    direction: str = "long_only"  # "long_only" | "long_short"

    def composite(self, signals: dict) -> float:
        """Weighted average of active sources; inactive (weight 0) auto-excluded."""
        active = {s: w for s, w in self.weights.items() if w > 0}
        total_w = sum(active.values()) or 1e-9
        return sum(signals.get(s, 0.0) * w for s, w in active.items()) / total_w


STRATEGY_PRESETS: dict = {
    "conservative": StrategyConfig(
        name="conservative",
        weights={
            SignalSource.RULE: 0.35,
            SignalSource.ATR: 0.15,
            SignalSource.VOLUME: 0.20,
            SignalSource.MTF: 0.30,
            SignalSource.ML: 0.0,
            SignalSource.GAF: 0.0,
        },
        entry_threshold=0.75,
    ),
    "balanced": StrategyConfig(name="balanced"),
    "aggressive": StrategyConfig(
        name="aggressive",
        weights={
            SignalSource.RULE: 0.60,
            SignalSource.ATR: 0.10,
            SignalSource.VOLUME: 0.30,
            SignalSource.MTF: 0.0,
            SignalSource.ML: 0.0,
            SignalSource.GAF: 0.0,
        },
        entry_threshold=0.55,
    ),
    "ml_blended": StrategyConfig(
        name="ml_blended",
        weights={
            SignalSource.RULE: 0.25,
            SignalSource.ATR: 0.10,
            SignalSource.VOLUME: 0.10,
            SignalSource.MTF: 0.20,
            SignalSource.ML: 0.35,
            SignalSource.GAF: 0.0,
        },
        entry_threshold=0.62,
    ),
}


def resolve_strategy(name_or_cfg) -> StrategyConfig:
    """Accept a preset name, a dict, or a StrategyConfig and return a config."""
    if isinstance(name_or_cfg, StrategyConfig):
        return name_or_cfg
    if isinstance(name_or_cfg, str):
        return STRATEGY_PRESETS.get(name_or_cfg, STRATEGY_PRESETS["balanced"])
    if isinstance(name_or_cfg, dict):
        base = STRATEGY_PRESETS.get(name_or_cfg.get("name", "balanced"), STRATEGY_PRESETS["balanced"])
        weights = dict(base.weights)
        for k, v in (name_or_cfg.get("weights") or {}).items():
            try:
                weights[SignalSource(k)] = float(v)
            except (ValueError, TypeError):
                continue
        return StrategyConfig(
            name=name_or_cfg.get("name", base.name),
            weights=weights,
            enabled_patterns=name_or_cfg.get("enabled_patterns", list(base.enabled_patterns)),
            entry_threshold=float(name_or_cfg.get("entry_threshold", base.entry_threshold)),
            direction=name_or_cfg.get("direction", base.direction),
        )
    return STRATEGY_PRESETS["balanced"]
