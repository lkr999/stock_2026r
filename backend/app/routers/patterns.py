"""Pattern detection endpoints (single, multi-scan, MTF)."""

import asyncio

from fastapi import APIRouter, Body, Depends, HTTPException, Query

from app.dependencies import get_detector, get_fetcher, get_token
from app.services.mtf_engine import MTFEngine
from app.services.pattern_detector import apply_strategy
from app.services.strategy import resolve_strategy
from app.services.timeframe import Timeframe
from app.services.universe import name_for

router = APIRouter()


def _tf(tf: str) -> Timeframe:
    try:
        return Timeframe(tf)
    except ValueError:
        raise HTTPException(400, f"invalid timeframe: {tf}")


@router.get("/{shcode}")
async def get_patterns(
    shcode: str,
    tf: str = Query("5m"),
    strategy: str = Query("balanced"),
    mtf: bool = Query(False),
    fetcher=Depends(get_fetcher),
    detector=Depends(get_detector),
    token: str = Depends(get_token),
):
    timeframe = _tf(tf)
    cfg = resolve_strategy(strategy)
    candles = await fetcher.fetch(token, shcode, timeframe)
    results = detector.scan(candles, timeframe, strategy=cfg)

    if mtf and results:
        engine = MTFEngine(fetcher, detector)
        for r in results:
            r.mtf_score = await engine.score(token, shcode, timeframe, r.pattern_type)
            apply_strategy(r, cfg)
        results.sort(key=lambda r: r.composite_score, reverse=True)

    return {
        "shcode": shcode,
        "name": name_for(shcode),
        "timeframe": tf,
        "patterns": [r.to_dict() for r in results],
    }


@router.post("/scan")
async def scan_many(
    body: dict = Body(...),
    fetcher=Depends(get_fetcher),
    detector=Depends(get_detector),
    token: str = Depends(get_token),
):
    codes = body.get("codes", [])[:50]
    timeframe = _tf(body.get("tf", "5m"))
    cfg = resolve_strategy(body.get("strategy", "balanced"))
    sem = asyncio.Semaphore(5)

    async def scan_one(code: str):
        async with sem:
            try:
                candles = await fetcher.fetch(token, code, timeframe)
                results = detector.scan(candles, timeframe, strategy=cfg)
                return {"shcode": code, "name": name_for(code),
                        "patterns": [r.to_dict() for r in results]}
            except Exception:
                return {"shcode": code, "name": name_for(code), "patterns": []}

    return await asyncio.gather(*[scan_one(c) for c in codes])
