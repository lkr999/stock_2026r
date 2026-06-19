import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from app.services.journal import TradeJournal, TradeRecord
from app.services.validation import ReadinessCriteria, evaluate_readiness


def _journal() -> TradeJournal:
    tmp = Path(tempfile.mkdtemp()) / "journal.json"
    return TradeJournal(path=tmp)


def _rec(code: str, ret_pct: float, days_ago: float) -> TradeRecord:
    closed = (datetime.now(timezone.utc) - timedelta(days=days_ago)).isoformat()
    pnl = ret_pct * 1000
    return TradeRecord(
        mode="paper", code=code, qty=10, entry=10000, exit=10000 * (1 + ret_pct / 100),
        pnl=pnl, return_pct=ret_pct, reason="take_profit", pattern="bullish_engulfing",
        opened_at=closed, closed_at=closed,
    )


def test_not_ready_when_no_trades():
    j = _journal()
    report = evaluate_readiness(j, ReadinessCriteria())
    assert not report.ready
    assert report.stats["trades"] == 0


def test_not_ready_with_few_trades():
    j = _journal()
    for i in range(5):
        j.record(_rec(f"00{i}", 1.0, days_ago=20 - i))
    report = evaluate_readiness(j, ReadinessCriteria())
    assert not report.ready
    failed = {c.key for c in report.criteria if not c.passed}
    assert "trades" in failed


def test_ready_when_all_criteria_met():
    j = _journal()
    # 35 trades over ~17 days, mostly winners -> PF>1.3, win_rate>0.45, positive pnl
    for i in range(35):
        ret = 2.0 if i % 3 != 0 else -1.0  # ~67% win, PF = (24*2)/(11*1) ~4.4
        j.record(_rec(f"c{i:03d}", ret, days_ago=25 - (i * 0.5)))
    report = evaluate_readiness(j, ReadinessCriteria())
    assert report.ready, [vars(c) for c in report.criteria if not c.passed]


def test_live_trades_excluded_from_paper_gate():
    j = _journal()
    j.record(TradeRecord(mode="live", code="x", qty=1, entry=1, exit=2, pnl=1,
                         return_pct=100, reason="tp", pattern="", opened_at="", closed_at=""))
    report = evaluate_readiness(j, ReadinessCriteria())
    assert report.stats["trades"] == 0  # live trade not counted as paper validation
