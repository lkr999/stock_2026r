"""Risk management gate (spec section 10-3).

Every entry must pass can_enter() + position_size(). Stops/targets are ATR-based
with a trailing stop, and a daily loss limit halts new entries for the day.
"""

from dataclasses import dataclass


@dataclass
class RiskConfig:
    max_position_pct: float = 0.10
    max_positions: int = 5
    risk_per_trade_pct: float = 0.01
    stop_loss_atr_mult: float = 1.5
    take_profit_atr_mult: float = 3.0
    daily_loss_limit_pct: float = 0.03
    trailing_stop_atr: float = 2.0


class RiskManager:
    def __init__(self, cfg: RiskConfig) -> None:
        self.cfg = cfg
        self._daily_pnl: float = 0.0
        self._open_positions: dict[str, dict] = {}

    @property
    def open_positions(self) -> dict[str, dict]:
        return self._open_positions

    @property
    def daily_pnl(self) -> float:
        return self._daily_pnl

    def can_enter(self, equity: float) -> tuple[bool, str]:
        if self._daily_pnl <= -equity * self.cfg.daily_loss_limit_pct:
            return False, "daily_loss_limit_reached"
        if len(self._open_positions) >= self.cfg.max_positions:
            return False, "max_positions_reached"
        return True, "ok"

    def position_size(self, equity: float, entry: float, atr: float) -> int:
        stop_distance = atr * self.cfg.stop_loss_atr_mult
        if stop_distance <= 0 or entry <= 0:
            return 0
        risk_amount = equity * self.cfg.risk_per_trade_pct
        qty_by_risk = int(risk_amount / stop_distance)
        qty_by_cap = int(equity * self.cfg.max_position_pct / entry)
        return max(0, min(qty_by_risk, qty_by_cap))

    def stop_and_target(self, entry: float, atr: float) -> tuple[float, float]:
        stop = entry - atr * self.cfg.stop_loss_atr_mult
        target = entry + atr * self.cfg.take_profit_atr_mult
        return stop, target

    def register(self, code: str, entry: float, qty: int, stop: float, target: float) -> None:
        self._open_positions[code] = {
            "entry": entry,
            "qty": qty,
            "stop": stop,
            "target": target,
            "peak": entry,
        }

    def check_exit(self, code: str, price: float, atr: float) -> str | None:
        pos = self._open_positions.get(code)
        if not pos:
            return None
        pos["peak"] = max(pos["peak"], price)
        trail = pos["peak"] - atr * self.cfg.trailing_stop_atr
        if price <= pos["stop"]:
            return "stop_loss"
        if price >= pos["target"]:
            return "take_profit"
        if price <= trail and price > pos["entry"]:
            return "trailing_stop"
        return None

    def on_close(self, code: str, pnl: float) -> None:
        self._daily_pnl += pnl
        self._open_positions.pop(code, None)

    def reduce(self, code: str, qty_sold: int, pnl: float) -> bool:
        """부분 청산. 보유수량을 차감하고, 0 이하가 되면 포지션을 제거한다.
        반환값: 포지션이 완전 청산되었으면 True."""
        pos = self._open_positions.get(code)
        if not pos:
            return True
        self._daily_pnl += pnl
        pos["qty"] -= qty_sold
        if pos["qty"] <= 0:
            self._open_positions.pop(code, None)
            return True
        return False

    def reset_daily(self) -> None:
        self._daily_pnl = 0.0
