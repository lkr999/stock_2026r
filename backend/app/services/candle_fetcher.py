"""Timeframe-aware OHLCV fetcher wrapping EBestService (spec section 2-3).

Provides one unified `fetch()` that routes daily candles to t8451 and intraday
candles to t8452, normalising both into the same OHLCV dict shape.
"""

import asyncio
import logging
import time
from typing import TYPE_CHECKING, Any

from app.services.timeframe import TIMEFRAME_CONFIGS, Timeframe

if TYPE_CHECKING:
    from app.services.ebest_service import EBestService

logger = logging.getLogger(__name__)


def _f(value: Any) -> float:
    try:
        return float(value or 0)
    except (TypeError, ValueError):
        return 0.0


class CandleFetcher:
    def __init__(self, ebest: "EBestService") -> None:
        self._ebest = ebest
        self._cache: dict[tuple, tuple[float, list[dict]]] = {}
        self._inflight: dict[tuple, asyncio.Task[list[dict]]] = {}
        self._lock = asyncio.Lock()

    async def fetch(
        self,
        token: str,
        shcode: str,
        tf: Timeframe,
        sdate: str = "",
        stime: str = "",
        edate: str = "",
        etime: str = "",
    ) -> list[dict]:
        key = (shcode, tf.value, sdate, stime, edate, etime)
        ttl = self._cache_ttl(tf)
        now = time.monotonic()

        async with self._lock:
            cached = self._cache.get(key)
            if cached and cached[0] > now:
                return [dict(r) for r in cached[1]]

            task = self._inflight.get(key)
            owner = task is None
            if task is None:
                task = asyncio.create_task(
                    self._fetch_uncached(token, shcode, tf, sdate, stime, edate, etime)
                )
                self._inflight[key] = task

        try:
            rows = await task
        finally:
            if owner:
                async with self._lock:
                    if self._inflight.get(key) is task:
                        self._inflight.pop(key, None)

        if owner:
            async with self._lock:
                self._cache[key] = (time.monotonic() + ttl, [dict(r) for r in rows])
                if len(self._cache) > 1000:
                    cutoff = time.monotonic()
                    self._cache = {
                        k: v for k, v in self._cache.items() if v[0] > cutoff
                    }

        return [dict(r) for r in rows]

    @staticmethod
    def _cache_ttl(tf: Timeframe) -> float:
        if tf == Timeframe.M1:
            return 35.0
        if tf in {Timeframe.M3, Timeframe.M5}:
            return 90.0
        if tf == Timeframe.D1:
            return 600.0
        return 180.0

    async def _fetch_uncached(
        self,
        token: str,
        shcode: str,
        tf: Timeframe,
        sdate: str = "",
        stime: str = "",
        edate: str = "",
        etime: str = "",
    ) -> list[dict]:
        """Return unified OHLCV rows (ascending) for the given timeframe."""
        cfg = TIMEFRAME_CONFIGS[tf]

        if tf == Timeframe.D1:
            rows = await self._ebest.fetch_daily_candles(token, shcode, qrycnt=cfg.qrycnt)
            # daily rows already have date/ohlcv; add ts for uniformity.
            for r in rows:
                r.setdefault("time", "")
                r["ts"] = f"{r.get('date', '')} {r.get('time', '')}".strip()
            return rows

        raw = await self._ebest.fetch_candle_data_tr(
            token=token,
            shcode=shcode,
            ncnt=cfg.ncnt,
            qrycnt=cfg.qrycnt,
            nday="0",  # qrycnt 만큼 최근 봉을 끌어온다 (nday="1"은 당일 봉만 → 데이터 부족)
            sdate=sdate,
            stime=stime,
            edate=edate,
            etime=etime,
        )
        return self._normalize_t8452(raw)

    @staticmethod
    def _normalize_t8452(raw: dict) -> list[dict]:
        rows: list[dict] = []
        for item in raw.get("t8452OutBlock1") or []:
            date = str(item.get("date", "")).strip()
            time = str(item.get("time", "")).strip()
            row = {
                "date": date,
                "time": time,
                "ts": f"{date} {time}".strip(),
                "open": _f(item.get("open")),
                "high": _f(item.get("high")),
                "low": _f(item.get("low")),
                "close": _f(item.get("close")),
                "volume": _f(item.get("jdiff_vol")),
            }
            if row["high"] > 0 and row["low"] > 0:
                rows.append(row)
        rows.sort(key=lambda r: r["ts"])
        return rows
