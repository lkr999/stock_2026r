"""Automated trading orchestrator (spec section 10-4).

Polls a watchlist each cycle, checks exits before entries, and routes orders
through Broker after passing the RiskManager gate. Market-hours aware.
"""

import asyncio
import logging
from dataclasses import dataclass
from datetime import datetime, time as dtime, timezone, timedelta

from app.services.broker import Broker
from app.services.candle_fetcher import CandleFetcher
from app.services.journal import TradeJournal, TradeRecord, now_iso
from app.services.pattern_detector import PatternDetector, compute_atr, to_candles
from app.services.risk import RiskManager
from app.services.strategy import StrategyConfig
from app.services.timeframe import Timeframe
from app.services.universe import name_for

logger = logging.getLogger(__name__)

KST = timezone(timedelta(hours=9))
KST_OPEN = dtime(9, 0)
KST_CLOSE = dtime(15, 20)


@dataclass
class OrderConfig:
    """주문 실행 설정 (1회 수량·주문유형·전량매도·매수 한도액)."""
    order_type: str = "limit"          # limit | market | best
    fixed_qty: int | None = None       # 1회 매수/매도 수량 (None=리스크 기반 자동)
    sell_all: bool = True              # 매도 시 보유 전량
    max_buy_amount: float = 500_000    # 1회 매수 한도액(원)

    def to_dict(self) -> dict:
        return {
            "order_type": self.order_type,
            "fixed_qty": self.fixed_qty or 0,
            "sell_all": self.sell_all,
            "max_buy_amount": self.max_buy_amount,
        }


def _candle_unix_ts(candle: dict) -> int:
    """마지막 캔들의 date/time 필드를 UTC Unix 초로 변환 (차트 마커 시간 맞춤)."""
    date = str(candle.get("date", "")).replace("-", "")
    time_s = str(candle.get("time", "")).replace(":", "")
    if len(date) == 8:
        try:
            hh = int(time_s[:2]) if len(time_s) >= 4 else 9
            mm = int(time_s[2:4]) if len(time_s) >= 4 else 0
            dt = datetime(
                int(date[:4]), int(date[4:6]), int(date[6:8]), hh, mm,
                tzinfo=timezone.utc
            )
            return int(dt.timestamp())
        except (ValueError, OverflowError):
            pass
    return int(datetime.now(timezone.utc).timestamp())


class TradingEngine:
    def __init__(
        self,
        ebest,
        fetcher: CandleFetcher,
        detector: PatternDetector,
        broker: Broker,
        strategy: StrategyConfig,
        risk: RiskManager,
        watchlist: list[str],
        tf: Timeframe = Timeframe.M5,
        ignore_market_hours: bool = False,
        journal: TradeJournal | None = None,
        seed_cash: float = 10_000_000,
        order: OrderConfig | None = None,
    ) -> None:
        self.ebest = ebest
        self.fetcher = fetcher
        self.detector = detector
        self.broker = broker
        self.strategy = strategy
        self.risk = risk
        self.watchlist = watchlist
        self.tf = tf
        self.ignore_market_hours = ignore_market_hours
        self.journal = journal
        self.seed_cash = seed_cash
        self.order = order or OrderConfig()
        # 자산/현금 추적은 두 모드 공통 (모드 차이는 Broker의 주문 전달뿐).
        self.cash = seed_cash
        self._running = False
        self._task = None
        self.last_error = None
        self.cycles = 0
        self._opened_meta: dict[str, dict] = {}  # code -> {opened_at, pattern}
        # 실시간 현재가 (차트 미실현손익 계산용)
        self._current_prices: dict[str, float] = {}
        # 거래 이벤트 (차트 마커 + 거래내역 표시용, 최대 500건 메모리 유지)
        self._trade_events: list[dict] = []

    @property
    def running(self) -> bool:
        return self._running

    def _market_open(self) -> bool:
        if self.ignore_market_hours:
            return True
        now = datetime.now(KST).time()
        return KST_OPEN <= now <= KST_CLOSE

    def _equity(self) -> float:
        """현재 자산 = 현금 + 보유 포지션 평가액(현재가 기준). 두 모드 동일 산식."""
        held = sum(
            p["qty"] * self._current_prices.get(code, p["entry"])
            for code, p in self.risk.open_positions.items()
        )
        return self.cash + held

    async def _seed_equity(self, token: str) -> None:
        """루프 시작 시 시드 현금 설정. PAPER=가상 시드, LIVE=실계좌 예수금."""
        if self.broker.mode.value == "live" and self.ebest is not None:
            try:
                bal = await self.ebest.get_account_balance(token)
                block = bal.get("t0424OutBlock", {}) if isinstance(bal, dict) else {}
                val = float(block.get("sunamt", 0) or 0)
                self.cash = val or self.seed_cash
                return
            except Exception:
                pass
        self.cash = self.seed_cash

    async def _resolve_token(self) -> str:
        """eBest 인증 토큰 발급. 토큰 없이는 데이터 조회/주문 불가."""
        if self.ebest is not None:
            return await self.ebest.auth_token() or ""
        return ""

    def _append_event(self, event: dict) -> None:
        self._trade_events.append(event)
        if len(self._trade_events) > 500:
            self._trade_events = self._trade_events[-500:]

    def clear_events(self) -> int:
        """이번 세션 거래 이벤트(차트 마커/세션 거래표)를 비운다. 삭제 건수 반환."""
        n = len(self._trade_events)
        self._trade_events = []
        return n

    async def manual_close(self, code: str) -> dict:
        """보유 포지션을 수동으로 전량 청산(매도)한다.

        자동 청산(손절/익절)과 동일한 경로로 매도 주문을 보내고, 현금·저널·세션
        이벤트를 갱신한 뒤 포지션을 제거한다. 모드 차이는 Broker의 주문 전달뿐.
        """
        pos = self.risk.open_positions.get(code)
        if not pos:
            return {"ok": False, "reason": "no_position"}
        token = await self._resolve_token()
        # 현재가 조회 (실패 시 최근 보관 현재가 → 진입가 순으로 폴백)
        price = self._current_prices.get(code, pos["entry"])
        candle_ts = int(datetime.now(timezone.utc).timestamp())
        try:
            candles = await self.fetcher.fetch(token, code, self.tf)
            if candles:
                price = to_candles(candles)[-1].close
                candle_ts = _candle_unix_ts(candles[-1])
        except Exception as exc:
            logger.warning("manual_close fetch failed %s: %s", code, exc)

        # 청산 직전 재확인 (루프와의 경합 방지)
        pos = self.risk.open_positions.get(code)
        if not pos:
            return {"ok": False, "reason": "no_position"}

        # 수량/진입가는 reduce()가 pos dict를 차감하기 전에 캡처해 둔다.
        qty = pos["qty"]
        entry = pos["entry"]
        res = await self.broker.sell(token, code, qty, price, order_type=self.order.order_type)
        if not res.get("ok"):
            return {"ok": False, "reason": res.get("reason", "order_failed")}
        fill = res.get("fill_price", price)
        self.cash += qty * fill
        pnl = (fill - entry) * qty
        pnl_pct = (fill - entry) / entry * 100 if entry else 0.0
        self._record_close(code, pos, fill, "manual_close", qty=qty, fully_closed=True)
        self.risk.reduce(code, qty, pnl)
        self._append_event({
            "code": code, "name": name_for(code),
            "type": "sell", "price": fill, "qty": qty,
            "pnl": round(pnl, 0), "pnl_pct": round(pnl_pct, 2),
            "reason": "manual_close", "ts": candle_ts,
            "time_label": datetime.now(KST).strftime("%H:%M"),
        })
        logger.info("MANUAL CLOSE %s x%d @%.0f pnl=%.0f", code, qty, fill, pnl)
        return {"ok": True, "fill_price": fill, "qty": qty, "pnl": round(pnl, 0),
                "pnl_pct": round(pnl_pct, 2)}

    async def _scan_and_trade(self, token: str) -> None:
        equity = self._equity()
        for code in self.watchlist:
            try:
                candles = await self.fetcher.fetch(token, code, self.tf)
            except Exception as exc:
                logger.warning("fetch failed %s: %s", code, exc)
                continue
            if len(candles) < 20:
                continue
            cobjs = to_candles(candles)
            atr = compute_atr(cobjs)
            price = cobjs[-1].close  # 현재가
            self._current_prices[code] = price
            candle_ts = _candle_unix_ts(candles[-1])

            # ── 청산(매도) 먼저 ── 두 모드 동일, 주문 전달만 Broker에서 갈림
            reason = self.risk.check_exit(code, price, atr)
            if reason:
                pos = self.risk.open_positions[code]
                held = pos["qty"]
                # 전량매도 또는 1회수량 미설정 → 전량, 아니면 지정 수량만큼 부분 매도
                sell_qty = held if (self.order.sell_all or not self.order.fixed_qty) \
                    else min(self.order.fixed_qty, held)
                res = await self.broker.sell(token, code, sell_qty, price, order_type=self.order.order_type)
                if res.get("ok"):
                    fill = res.get("fill_price", price)
                    self.cash += sell_qty * fill
                    pnl = (fill - pos["entry"]) * sell_qty
                    pnl_pct = (fill - pos["entry"]) / pos["entry"] * 100 if pos["entry"] else 0
                    closed = self.risk.reduce(code, sell_qty, pnl)
                    self._record_close(code, pos, fill, reason, qty=sell_qty, fully_closed=closed)
                    self._append_event({
                        "code": code, "name": name_for(code),
                        "type": "sell", "price": fill, "qty": sell_qty,
                        "pnl": round(pnl, 0), "pnl_pct": round(pnl_pct, 2),
                        "reason": reason, "ts": candle_ts,
                        "time_label": datetime.now(KST).strftime("%H:%M"),
                    })
                    logger.info("EXIT %s (%s) x%d pnl=%.0f%s", code, reason, sell_qty, pnl,
                                "" if closed else " (부분청산)")
                continue

            # 이미 보유 중이면 신규 진입 건너뜀
            if code in self.risk.open_positions:
                continue

            # ── 진입(매수) 판정 ── 시그널 → 게이트 → 수량 (두 모드 동일)
            results = self.detector.scan(candles, self.tf, strategy=self.strategy)
            cands = [
                r for r in results
                if r.pattern_name in self.strategy.enabled_patterns
                and r.composite_score >= self.strategy.entry_threshold
            ]
            if not cands:
                continue
            ok, why = self.risk.can_enter(equity)
            if not ok:
                logger.info("entry blocked %s: %s", code, why)
                continue
            # 1회 수량: 고정수량 지정 시 사용, 아니면 리스크 기반 자동 산정
            qty = self.order.fixed_qty if self.order.fixed_qty else self.risk.position_size(equity, price, atr)
            # 매수 한도액 / 가용 현금 상한 적용
            if self.order.max_buy_amount > 0:
                qty = min(qty, int(self.order.max_buy_amount / price))
            qty = min(qty, int(self.cash / price))
            if qty <= 0:
                continue
            res = await self.broker.buy(token, code, qty, price, order_type=self.order.order_type)
            if res.get("ok"):
                fill = res.get("fill_price", price)
                self.cash -= qty * fill
                stop, target = self.risk.stop_and_target(fill, atr)
                self.risk.register(code, fill, qty, stop, target)
                meta = {"opened_at": now_iso(), "pattern": cands[0].pattern_name}
                self._opened_meta[code] = meta
                self._append_event({
                    "code": code, "name": name_for(code),
                    "type": "buy", "price": fill, "qty": qty,
                    "pnl": 0.0, "pnl_pct": 0.0,
                    "reason": cands[0].pattern_name, "ts": candle_ts,
                    "time_label": datetime.now(KST).strftime("%H:%M"),
                })
                logger.info(
                    "ENTER %s x%d @%.0f stop=%.0f target=%.0f (%s %.2f)",
                    code, qty, fill, stop, target,
                    cands[0].pattern_name, cands[0].composite_score,
                )

    def _record_close(
        self, code: str, pos: dict, exit_price: float, reason: str,
        qty: int | None = None, fully_closed: bool = True,
    ) -> None:
        if self.journal is None:
            return
        entry = pos["entry"]
        sold_qty = qty if qty is not None else pos["qty"]
        # 메타(진입시각·패턴)는 완전 청산 시에만 제거. 부분청산은 보존.
        meta = self._opened_meta.pop(code, {}) if fully_closed else self._opened_meta.get(code, {})
        ret_pct = (exit_price - entry) / entry * 100 if entry else 0.0
        self.journal.record(TradeRecord(
            mode=self.broker.mode.value,
            code=code, qty=sold_qty, entry=entry, exit=exit_price,
            pnl=(exit_price - entry) * sold_qty, return_pct=ret_pct, reason=reason,
            pattern=meta.get("pattern", ""),
            opened_at=meta.get("opened_at", ""), closed_at=now_iso(),
        ))

    async def _loop(self, poll_sec: int) -> None:
        self._running = True
        self.risk.reset_daily()
        token = await self._resolve_token()
        await self._seed_equity(token)
        while self._running:
            if self._market_open():
                try:
                    await self._scan_and_trade(token)
                    self.cycles += 1
                    self.last_error = None
                except Exception as exc:
                    self.last_error = str(exc)
                    logger.error("trade loop error: %s", exc)
            await asyncio.sleep(poll_sec)

    def start(self, poll_sec: int = 60) -> None:
        if self._running:
            return
        self._task = asyncio.create_task(self._loop(poll_sec))

    def stop(self) -> None:
        self._running = False
        if self._task:
            self._task.cancel()
            self._task = None

    def status(self) -> dict:
        positions: list[dict] = []
        for code, p in self.risk.open_positions.items():
            cur = self._current_prices.get(code, p["entry"])
            upnl = (cur - p["entry"]) * p["qty"]
            upct = (cur - p["entry"]) / p["entry"] * 100 if p["entry"] else 0.0
            meta = self._opened_meta.get(code, {})
            positions.append({
                "code": code,
                "name": name_for(code),
                **p,
                # entry = 실제 매수 체결가(매수가). 이 포지션은 매수 시그널이 실제 실행된 결과.
                "buy_price": round(p["entry"], 0),
                "current_price": round(cur, 0),
                "unrealized_pnl": round(upnl, 0),
                "unrealized_pct": round(upct, 2),
                # 이 포지션을 만든 실제 매수 시그널 기록
                "pattern": meta.get("pattern", ""),
                "opened_at": meta.get("opened_at", ""),
            })
        return {
            "running": self._running,
            "mode": self.broker.mode.value,
            "strategy": self.strategy.name,
            "timeframe": self.tf.value,
            "watchlist": self.watchlist,
            "cycles": self.cycles,
            "cash": round(self.cash, 0),
            "equity": round(self._equity(), 0),
            "daily_pnl": round(self.risk.daily_pnl, 0),
            "positions": positions,
            "trade_events": self._trade_events[-100:],
            "order": self.order.to_dict(),
            "last_error": self.last_error,
        }
