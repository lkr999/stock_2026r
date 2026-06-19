"""Watchlist / universe signal scan endpoint."""

import asyncio

from fastapi import APIRouter, Body, Depends, Query

from app.dependencies import get_detector, get_fetcher, get_token
from app.services.strategy import resolve_strategy
from app.services.timeframe import Timeframe
from app.services.universe import filter_universe, name_for

router = APIRouter()
MAX_UNIVERSE_SCAN_LIMIT = 50


@router.get("")
async def list_signals(
    market: str = Query("ALL"),
    tf: str = Query("5m"),
    strategy: str = Query("balanced"),
    min_conf: float = Query(0.6),
    limit: int = Query(30),
    scan_limit: int = Query(120, description="max universe symbols scanned per request"),
    recent_bars: int = Query(20, description="최근 N봉 내 형성된 패턴까지 탐지"),
    min_price: float | None = Query(None, description="현재가 하한 필터"),
    max_price: float | None = Query(None, description="현재가 상한 필터"),
    fetcher=Depends(get_fetcher),
    detector=Depends(get_detector),
    token: str = Depends(get_token),
):
    timeframe = Timeframe(tf)
    cfg = resolve_strategy(strategy)
    scan_limit = max(1, min(scan_limit, MAX_UNIVERSE_SCAN_LIMIT))
    recent_bars = max(1, min(recent_bars, 60))
    # 전 종목을 대상으로 하되 현재가 기준으로 먼저 필터링한 뒤 스캔한다.
    stocks = filter_universe(market=market, min_price=min_price, max_price=max_price)[:scan_limit]
    sem = asyncio.Semaphore(5)

    async def scan_one(stock: dict):
        async with sem:
            try:
                candles = await fetcher.fetch(token, stock["code"], timeframe)
                patterns = detector.scan(
                    candles, timeframe, min_confidence=min_conf,
                    strategy=cfg, recent_bars=recent_bars,
                )
                return stock, patterns
            except Exception:
                return stock, []

    results = await asyncio.gather(*[scan_one(s) for s in stocks])
    signals: list = []
    for stock, patterns in results:
        for p in patterns:
            if p.composite_score < cfg.entry_threshold:
                continue
            signals.append({
                "shcode": stock["code"],
                "name": stock["name"] or name_for(stock["code"]),
                "market": stock["market"],
                "price": stock.get("price", 0),
                "change_pct": stock.get("change_pct", 0),
                "pattern_name": p.pattern_name,
                "pattern_type": p.pattern_type,
                "confidence": p.confidence,
                "composite_score": p.composite_score,
                "atr_normalized": p.atr_normalized,
                "volume_confirmed": p.volume_confirmed,
                "detected_at": p.detected_at,
                "expected_5": p.expected_5,
            })
    signals.sort(key=lambda x: x["composite_score"], reverse=True)
    return signals[:limit]


@router.post("/watchlist")
async def scan_watchlist(
    body: dict = Body(...),
    fetcher=Depends(get_fetcher),
    detector=Depends(get_detector),
    token: str = Depends(get_token),
):
    codes = body.get("codes", [])
    timeframe = Timeframe(body.get("tf", "5m"))
    cfg = resolve_strategy(body.get("strategy", "balanced"))
    min_conf = float(body.get("min_conf", 0.6))
    recent_bars = max(1, min(int(body.get("recent_bars", 20)), 60))
    sem = asyncio.Semaphore(5)

    async def scan_one(code: str):
        async with sem:
            try:
                candles = await fetcher.fetch(token, code, timeframe)
                patterns = detector.scan(
                    candles, timeframe, min_confidence=min_conf,
                    strategy=cfg, recent_bars=recent_bars,
                )
                return code, patterns
            except Exception:
                return code, []

    results = await asyncio.gather(*[scan_one(c) for c in codes])
    signals: list = []
    for code, patterns in results:
        for p in patterns:
            if p.composite_score < cfg.entry_threshold:
                continue
            signals.append({
                "shcode": code, "name": name_for(code),
                "pattern_name": p.pattern_name, "pattern_type": p.pattern_type,
                "confidence": p.confidence, "composite_score": p.composite_score,
                "detected_at": p.detected_at,
            })
    signals.sort(key=lambda x: x["composite_score"], reverse=True)
    return signals
