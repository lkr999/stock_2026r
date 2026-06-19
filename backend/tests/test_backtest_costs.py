from __future__ import annotations

from app.services.backtest_engine import CostModel, run_backtest
from app.services.timeframe import Timeframe


def test_round_trip_cost_components():
    c = CostModel()
    # 2*0.00015 + 0.0015 + 2*0.0008 = 0.0034
    assert abs(c.round_trip_cost() - 0.0034) < 1e-9


def test_backtest_returns_zero_signals_on_flat_data():
    candles = [{"ts": f"b{i}", "open": 100, "high": 101, "low": 99, "close": 100, "volume": 1000}
               for i in range(60)]
    res = run_backtest(candles, "bullish_engulfing", Timeframe.D1, hold_bars=5)
    assert res["signals"] == 0
    assert res["avg_return"] == 0.0


def test_cost_reduces_return():
    # Without cost a +1% move; cost model must subtract round-trip cost.
    free = CostModel(fee_rate=0, tax_rate=0, slippage_rate=0)
    paid = CostModel()
    assert paid.round_trip_cost() > free.round_trip_cost()
