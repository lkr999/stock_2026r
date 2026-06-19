from __future__ import annotations

from app.services.risk import RiskConfig, RiskManager


def test_position_size_respects_risk_per_trade():
    rm = RiskManager(RiskConfig(risk_per_trade_pct=0.01, stop_loss_atr_mult=1.5, max_position_pct=1.0))
    qty = rm.position_size(equity=10_000_000, entry=50_000, atr=1000)
    # risk_amount = 100,000 ; stop_distance = 1500 -> 66 shares
    assert qty == 66


def test_position_size_capped_by_max_position_pct():
    rm = RiskManager(RiskConfig(risk_per_trade_pct=0.50, max_position_pct=0.10, stop_loss_atr_mult=1.5))
    qty = rm.position_size(equity=10_000_000, entry=50_000, atr=1000)
    # cap = 10,000,000*0.10/50,000 = 20
    assert qty == 20


def test_daily_loss_limit_blocks_entry():
    rm = RiskManager(RiskConfig(daily_loss_limit_pct=0.03))
    rm.on_close("A", pnl=-400_000)  # -4% of 10M
    ok, why = rm.can_enter(10_000_000)
    assert not ok and why == "daily_loss_limit_reached"


def test_max_positions_blocks_entry():
    rm = RiskManager(RiskConfig(max_positions=1))
    rm.register("A", 100, 1, 90, 120)
    ok, why = rm.can_enter(10_000_000)
    assert not ok and why == "max_positions_reached"


def test_stop_loss_exit():
    rm = RiskManager(RiskConfig(stop_loss_atr_mult=1.5))
    rm.register("A", entry=10_000, qty=10, stop=8_500, target=13_000)
    assert rm.check_exit("A", price=8_400, atr=1000) == "stop_loss"


def test_take_profit_exit():
    rm = RiskManager(RiskConfig())
    rm.register("A", entry=10_000, qty=10, stop=8_500, target=13_000)
    assert rm.check_exit("A", price=13_100, atr=1000) == "take_profit"


def test_trailing_stop_exit():
    rm = RiskManager(RiskConfig(trailing_stop_atr=2.0))
    rm.register("A", entry=10_000, qty=10, stop=8_500, target=20_000)
    rm.check_exit("A", price=12_000, atr=500)   # peak -> 12000, trail -> 11000
    assert rm.check_exit("A", price=10_900, atr=500) == "trailing_stop"
