"""Persistent trade journal — records every closed trade to disk.

Used to build a paper-trading track record that the live-trading readiness gate
(validation.py) evaluates before allowing real orders.
"""

import json
import logging
import threading
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

DATA_DIR = Path(__file__).resolve().parents[2] / "data"
JOURNAL_PATH = DATA_DIR / "trade_journal.json"


@dataclass
class TradeRecord:
    mode: str  # "paper" | "live"
    code: str
    qty: int
    entry: float
    exit: float
    pnl: float
    return_pct: float
    reason: str  # exit reason
    pattern: str
    opened_at: str
    closed_at: str


class TradeJournal:
    """Append-only journal persisted as a JSON array. Thread-safe."""

    def __init__(self, path: Path = JOURNAL_PATH) -> None:
        self._path = path
        self._lock = threading.Lock()
        self._path.parent.mkdir(parents=True, exist_ok=True)
        if not self._path.exists():
            self._path.write_text("[]", encoding="utf-8")

    def record(self, trade: TradeRecord) -> None:
        with self._lock:
            data = self._read()
            data.append(asdict(trade))
            self._path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
        logger.info("[journal] %s %s pnl=%.0f (%.2f%%)", trade.mode, trade.code, trade.pnl, trade.return_pct)

    def all(self) -> list[dict[str, Any]]:
        with self._lock:
            return self._read()

    def by_mode(self, mode: str) -> list[dict[str, Any]]:
        return [t for t in self.all() if t.get("mode") == mode]

    def clear(self) -> None:
        with self._lock:
            self._path.write_text("[]", encoding="utf-8")

    def clear_mode(self, mode: str) -> int:
        """특정 모드(paper/live) 기록만 삭제. 삭제 건수를 반환."""
        with self._lock:
            data = self._read()
            kept = [t for t in data if t.get("mode") != mode]
            removed = len(data) - len(kept)
            self._path.write_text(json.dumps(kept, ensure_ascii=False, indent=2), encoding="utf-8")
        return removed

    def stats(self, mode: str | None = None) -> dict[str, Any]:
        """집계 통계: 승률, 총 손익, 손익비 등."""
        trades = self.all() if mode is None else self.by_mode(mode)
        if not trades:
            return {
                "count": 0, "win_count": 0, "loss_count": 0,
                "total_pnl": 0.0, "win_rate": 0.0, "profit_factor": None,
                "avg_return_pct": 0.0, "best_trade_pnl": 0.0, "worst_trade_pnl": 0.0,
            }
        wins = [t for t in trades if t.get("pnl", 0) > 0]
        losses = [t for t in trades if t.get("pnl", 0) <= 0]
        gross_profit = sum(t["pnl"] for t in wins)
        gross_loss = abs(sum(t["pnl"] for t in losses))
        pnls = [t["pnl"] for t in trades]
        rets = [t.get("return_pct", 0) for t in trades]
        return {
            "count": len(trades),
            "win_count": len(wins),
            "loss_count": len(losses),
            "total_pnl": round(sum(pnls), 0),
            "win_rate": round(len(wins) / len(trades) * 100, 1),
            "profit_factor": round(gross_profit / gross_loss, 2) if gross_loss > 0 else None,
            "avg_return_pct": round(sum(rets) / len(rets), 2),
            "best_trade_pnl": round(max(pnls), 0),
            "worst_trade_pnl": round(min(pnls), 0),
        }

    def _read(self) -> list[dict[str, Any]]:
        try:
            return json.loads(self._path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, FileNotFoundError):
            return []


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()
