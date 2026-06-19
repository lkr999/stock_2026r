"""Timeframe definitions and per-timeframe configuration (spec section 2)."""

from dataclasses import dataclass
from enum import Enum


class Timeframe(str, Enum):
    M1 = "1m"
    M3 = "3m"
    M5 = "5m"
    M10 = "10m"
    M15 = "15m"
    M30 = "30m"
    H1 = "60m"
    D1 = "1d"


@dataclass(frozen=True)
class TimeframeConfig:
    timeframe: Timeframe
    tr_code: str  # "t8452" | "t8451"
    ncnt: int  # minute unit for t8452 (0 for daily)
    qrycnt: int
    trend_lookback: int
    hold_bars: tuple[int, ...] = (5, 10, 25)
    label: str = ""


TIMEFRAME_CONFIGS: dict[Timeframe, TimeframeConfig] = {
    Timeframe.M1: TimeframeConfig(Timeframe.M1, "t8452", 1, 500, 20, (5, 10, 25), "1분"),
    Timeframe.M3: TimeframeConfig(Timeframe.M3, "t8452", 3, 500, 15, (5, 10, 25), "3분"),
    Timeframe.M5: TimeframeConfig(Timeframe.M5, "t8452", 5, 300, 12, (5, 10, 25), "5분"),
    Timeframe.M10: TimeframeConfig(Timeframe.M10, "t8452", 10, 200, 10, (5, 10, 25), "10분"),
    Timeframe.M15: TimeframeConfig(Timeframe.M15, "t8452", 15, 200, 10, (5, 10, 25), "15분"),
    Timeframe.M30: TimeframeConfig(Timeframe.M30, "t8452", 30, 100, 8, (5, 10, 25), "30분"),
    Timeframe.H1: TimeframeConfig(Timeframe.H1, "t8452", 60, 100, 8, (5, 10, 25), "60분"),
    Timeframe.D1: TimeframeConfig(Timeframe.D1, "t8451", 0, 60, 5, (5, 10, 25), "일봉"),
}

# Upper timeframes for MTF confluence (section 9-3).
MTF_GROUPS: dict[Timeframe, list[Timeframe]] = {
    Timeframe.M5: [Timeframe.M15, Timeframe.H1],
    Timeframe.M15: [Timeframe.H1, Timeframe.D1],
    Timeframe.M30: [Timeframe.H1, Timeframe.D1],
    Timeframe.H1: [Timeframe.D1],
    Timeframe.D1: [],
}


def config_for(tf: Timeframe) -> TimeframeConfig:
    return TIMEFRAME_CONFIGS[tf]
