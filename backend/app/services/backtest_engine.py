"""Cost-aware, look-ahead-safe backtester + walk-forward (spec sections 6-3, 11-1)."""

from dataclasses import dataclass

import numpy as np

from app.services.pattern_detector import PatternDetector
from app.services.timeframe import Timeframe


@dataclass
class CostModel:
    """Korean equity round-trip cost model."""
    fee_rate: float = 0.00015       # broker fee per side
    tax_rate: float = 0.0015        # sell-side transaction tax (2025: 0.15%)
    slippage_rate: float = 0.0008   # slippage per side

    def round_trip_cost(self) -> float:
        return (self.fee_rate * 2) + self.tax_rate + (self.slippage_rate * 2)


def run_backtest(
    candles: list[dict],
    pattern_name: str,
    tf: Timeframe,
    hold_bars: int,
    cost: CostModel | None = None,
    side: str = "long",
    detector: PatternDetector | None = None,
) -> dict:
    cost = cost or CostModel()
    detector = detector or PatternDetector()
    rt_cost = cost.round_trip_cost() * 100  # %
    returns: list[float] = []

    for i in range(len(candles) - hold_bars - 1):
        window = candles[max(0, i - 40): i + 1]
        if len(window) < 13:
            continue
        found = detector.scan(window, tf, use_modern=False)
        if pattern_name not in {p.pattern_name for p in found}:
            continue
        entry = candles[i + 1]["open"]
        if entry <= 0:
            continue
        exit_ = candles[i + 1 + hold_bars]["close"]
        gross = (exit_ - entry) / entry * 100
        if side == "short":
            gross = -gross
        returns.append(gross - rt_cost)

    if not returns:
        return {
            "pattern": pattern_name, "signals": 0, "win_rate": 0.0, "avg_return": 0.0,
            "max_drawdown": 0.0, "sharpe_ratio": 0.0, "profit_factor": 0.0,
        }

    arr = np.array(returns, dtype=float)
    wins = arr[arr > 0]
    losses = arr[arr <= 0]
    equity = np.cumsum(arr)
    peak = np.maximum.accumulate(equity)
    mdd = float(np.min(equity - peak)) if len(equity) else 0.0
    pf = float(wins.sum() / abs(losses.sum())) if losses.sum() != 0 else float("inf")

    return {
        "pattern": pattern_name,
        "signals": len(returns),
        "win_rate": float(len(wins) / len(arr)),
        "avg_return": float(arr.mean()),
        "max_drawdown": mdd,
        "sharpe_ratio": float(arr.mean() / (arr.std() + 1e-9)),
        "profit_factor": pf,
    }


def backtest_many(
    candles: list[dict],
    pattern_names: list[str],
    tf: Timeframe,
    hold_bars: int,
    cost: CostModel | None = None,
) -> dict:
    detector = PatternDetector()
    by_pattern = [run_backtest(candles, p, tf, hold_bars, cost, detector=detector) for p in pattern_names]
    total = sum(b["signals"] for b in by_pattern)
    if total == 0:
        avg = 0.0
        win = 0.0
    else:
        avg = sum(b["avg_return"] * b["signals"] for b in by_pattern) / total
        win = sum(b["win_rate"] * b["signals"] for b in by_pattern) / total
    return {
        "total_signals": total,
        "avg_return": avg,
        "win_rate": win,
        "by_pattern": by_pattern,
    }


# --------------------------------------------------------------------------
# walk-forward (section 11-1)
# --------------------------------------------------------------------------

def walk_forward_split(candles: list[dict], n_folds: int = 4):
    n = len(candles)
    fold_size = n // (n_folds + 1)
    if fold_size < 15:
        yield candles[: n // 2], candles[n // 2:]
        return
    for k in range(n_folds):
        train_end = fold_size * (k + 1)
        test_end = train_end + fold_size
        yield candles[:train_end], candles[train_end:test_end]


def evaluate_strategy(candles: list[dict], enabled_patterns: list[str], tf: Timeframe, hold_bars: int = 5) -> dict:
    cost = CostModel()
    oos_returns: list[float] = []
    for _train, test in walk_forward_split(candles):
        for pattern in enabled_patterns:
            r = run_backtest(test, pattern, tf, hold_bars, cost)
            if r["signals"] > 0:
                oos_returns.append(r["avg_return"])
    return {
        "oos_avg_return": float(np.mean(oos_returns)) if oos_returns else 0.0,
        "oos_consistency": float(np.mean([x > 0 for x in oos_returns])) if oos_returns else 0.0,
        "oos_samples": len(oos_returns),
    }
