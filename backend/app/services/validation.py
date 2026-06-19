"""Live-trading readiness gate (spec section 11-2 — 실전 투입 체크리스트).

Evaluates the persisted paper-trading track record against objective criteria.
Live trading is blocked until every required criterion passes, enforcing
"sufficient validation before going live".
"""

from dataclasses import dataclass, field
from datetime import datetime
from typing import Any

import numpy as np

from app.services.journal import TradeJournal


@dataclass
class ReadinessCriteria:
    min_paper_trades: int = 30            # enough samples to be meaningful
    min_paper_days: int = 14              # ≥2 weeks of paper trading (spec 11-2)
    min_win_rate: float = 0.45            # net-of-cost win rate
    min_profit_factor: float = 1.3        # spec threshold for a live candidate
    require_positive_pnl: bool = True     # cumulative paper P&L must be > 0


@dataclass
class CriterionResult:
    key: str
    label: str
    passed: bool
    actual: float
    required: float


@dataclass
class ReadinessReport:
    ready: bool
    criteria: list[CriterionResult] = field(default_factory=list)
    stats: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return {
            "ready": self.ready,
            "criteria": [vars(c) for c in self.criteria],
            "stats": self.stats,
        }


def _paper_stats(trades: list[dict[str, Any]]) -> dict[str, Any]:
    if not trades:
        return {"trades": 0, "win_rate": 0.0, "profit_factor": 0.0,
                "total_pnl": 0.0, "avg_return_pct": 0.0, "span_days": 0.0}
    rets = np.array([t.get("return_pct", 0.0) for t in trades], dtype=float)
    pnls = np.array([t.get("pnl", 0.0) for t in trades], dtype=float)
    wins = rets[rets > 0]
    losses = rets[rets <= 0]
    pf = float(wins.sum() / abs(losses.sum())) if losses.sum() != 0 else (float("inf") if wins.sum() > 0 else 0.0)

    times = []
    for t in trades:
        try:
            times.append(datetime.fromisoformat(t["closed_at"]))
        except (KeyError, ValueError):
            continue
    span_days = (max(times) - min(times)).total_seconds() / 86400 if len(times) >= 2 else 0.0

    return {
        "trades": len(trades),
        "win_rate": float(len(wins) / len(rets)),
        "profit_factor": pf,
        "total_pnl": float(pnls.sum()),
        "avg_return_pct": float(rets.mean()),
        "span_days": round(span_days, 2),
    }


def evaluate_readiness(journal: TradeJournal, criteria: ReadinessCriteria) -> ReadinessReport:
    trades = journal.by_mode("paper")
    s = _paper_stats(trades)

    checks = [
        CriterionResult("trades", "페이퍼 거래 수", s["trades"] >= criteria.min_paper_trades,
                        float(s["trades"]), float(criteria.min_paper_trades)),
        CriterionResult("days", "검증 기간(일)", s["span_days"] >= criteria.min_paper_days,
                        float(s["span_days"]), float(criteria.min_paper_days)),
        CriterionResult("win_rate", "승률", s["win_rate"] >= criteria.min_win_rate,
                        round(s["win_rate"], 3), criteria.min_win_rate),
        CriterionResult("profit_factor", "Profit Factor", s["profit_factor"] >= criteria.min_profit_factor,
                        round(s["profit_factor"], 2) if s["profit_factor"] != float("inf") else 999.0,
                        criteria.min_profit_factor),
    ]
    if criteria.require_positive_pnl:
        checks.append(CriterionResult("pnl", "누적 손익(>0)", s["total_pnl"] > 0,
                                      round(s["total_pnl"], 0), 0.0))

    return ReadinessReport(ready=all(c.passed for c in checks), criteria=checks, stats=s)
