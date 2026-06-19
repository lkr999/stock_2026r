"""주문 실행 계층 (spec section 10-2).

모의투자(PAPER)와 실전투자(LIVE)의 **유일한 차이는 주문 전달 방식**이다.
시그널 발생 → 리스크 게이트 통과 → 수량 결정까지는 두 모드가 완전히 동일하며,
오직 이 Broker.buy/sell 에서만 갈린다.

- PAPER : 시그널 발생 시 현재가로 즉시 체결(시뮬레이션). eBest 호출 없음.
- LIVE  : eBest REST API(CSPAT00601)로 실제 주문 전송.
"""

import logging
from enum import Enum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from app.services.ebest_service import EBestService

logger = logging.getLogger(__name__)


class TradingMode(str, Enum):
    PAPER = "paper"
    LIVE = "live"


# 주문유형 → eBest OrdprcPtnCode(호가유형코드)
#   00=지정가, 03=시장가, 06=최유리지정가
PRICE_TYPE_MAP = {
    "limit": "00",   # 지정가
    "market": "03",  # 시장가
    "best": "06",    # 최유리지정가
}


def resolve_price_type(order_type: str) -> str:
    return PRICE_TYPE_MAP.get(order_type, "00")


class Broker:
    def __init__(self, ebest: "EBestService | None", mode: TradingMode = TradingMode.PAPER) -> None:
        self._ebest = ebest
        self._mode = mode

    @property
    def mode(self) -> TradingMode:
        return self._mode

    async def buy(self, token: str, code: str, qty: int, price: float, order_type: str = "limit") -> dict:
        """매수. PAPER=현재가 체결, LIVE=eBest 주문(지정가/시장가/최유리지정가)."""
        if qty <= 0:
            return {"ok": False, "reason": "invalid_qty"}
        if self._mode == TradingMode.PAPER:
            logger.info("[PAPER] BUY %s x%d @%.0f (%s, 현재가 체결)", code, qty, price, order_type)
            return {"ok": True, "mode": "paper", "code": code, "qty": qty,
                    "fill_price": price, "order_type": order_type}
        return await self._live_order(token, code, "buy", qty, price, resolve_price_type(order_type))

    async def sell(self, token: str, code: str, qty: int, price: float, order_type: str = "limit") -> dict:
        """매도. PAPER=현재가 체결, LIVE=eBest 주문(지정가/시장가/최유리지정가)."""
        if qty <= 0:
            return {"ok": False, "reason": "invalid_qty"}
        if self._mode == TradingMode.PAPER:
            logger.info("[PAPER] SELL %s x%d @%.0f (%s, 현재가 체결)", code, qty, price, order_type)
            return {"ok": True, "mode": "paper", "code": code, "qty": qty,
                    "fill_price": price, "order_type": order_type}
        return await self._live_order(token, code, "sell", qty, price, resolve_price_type(order_type))

    async def _live_order(self, token, code, side, qty, price, price_type) -> dict:
        if self._ebest is None:
            logger.error("[LIVE] broker has no eBest connection — order skipped")
            return {"ok": False, "mode": "live", "reason": "no_broker_connection"}
        res = await self._ebest.place_order(token, code, side, qty, price, price_type=price_type)
        ok = str(res.get("rsp_cd", "")) == "0000"
        logger.info("[LIVE] %s %s x%d @%.0f type=%s -> %s %s", side.upper(), code, qty, price,
                    price_type, res.get("rsp_cd"), res.get("rsp_msg", ""))
        return {"ok": ok, "mode": "live", "code": code, "qty": qty, "fill_price": price, "raw": res}
