"""Backtest endpoints (cost-aware, walk-forward, preset comparison)."""

import logging

from fastapi import APIRouter, Body, Depends

from app.dependencies import get_detector, get_fetcher, get_token
from app.services.backtest_engine import (
    CostModel,
    backtest_many,
    evaluate_strategy,
    run_backtest,
)
from app.services.pattern_detector import PAPER_RETURNS
from app.services.strategy import STRATEGY_PRESETS, resolve_strategy
from app.services.timeframe import Timeframe
from app.services.universe import name_for

logger = logging.getLogger(__name__)

router = APIRouter()

ALL_PATTERNS = list(PAPER_RETURNS.keys())


def _cost(body: dict) -> CostModel:
    c = body.get("cost") or {}
    return CostModel(
        fee_rate=float(c.get("fee_rate", 0.00015)),
        tax_rate=float(c.get("tax_rate", 0.0015)),
        slippage_rate=float(c.get("slippage_rate", 0.0008)),
    )


@router.post("")
async def backtest(
    body: dict = Body(...),
    fetcher=Depends(get_fetcher),
    token: str = Depends(get_token),
):
    shcode = body["shcode"]
    tf = Timeframe(body.get("tf", "1d"))
    hold_bars = int(body.get("hold_bars", 5))
    patterns = body.get("pattern_names") or ALL_PATTERNS
    cost = _cost(body)

    candles = await fetcher.fetch(token, shcode, tf)
    result = backtest_many(candles, patterns, tf, hold_bars, cost)
    result["shcode"] = shcode
    result["timeframe"] = tf.value
    result["hold_bars"] = hold_bars
    result["round_trip_cost_pct"] = cost.round_trip_cost() * 100
    return result


@router.post("/batch")
async def backtest_batch(
    body: dict = Body(...),
    fetcher=Depends(get_fetcher),
    token: str = Depends(get_token),
):
    """관심종목(자동매매 대상) 전체에 대해 백테스트를 일괄 실행한다.

    body: { shcodes: [...], tf, hold_bars, strategy?, pattern_names?, cost? }
    strategy 가 주어지면 해당 프리셋의 enabled_patterns 만 사용한다.
    """
    shcodes: list[str] = [c.strip() for c in (body.get("shcodes") or []) if c and c.strip()]
    tf = Timeframe(body.get("tf", "1d"))
    hold_bars = int(body.get("hold_bars", 5))
    cost = _cost(body)

    # 패턴 집합: strategy 지정 시 프리셋 패턴, 아니면 명시 패턴/전체
    strategy_name = body.get("strategy")
    if strategy_name:
        patterns = list(resolve_strategy(strategy_name).enabled_patterns)
    else:
        patterns = body.get("pattern_names") or ALL_PATTERNS

    items: list[dict] = []
    for code in shcodes:
        try:
            candles = await fetcher.fetch(token, code, tf)
        except Exception as exc:
            logger.warning("batch backtest fetch failed %s: %s", code, exc)
            items.append({
                "shcode": code, "name": name_for(code), "ok": False,
                "error": f"데이터 조회 오류: {exc}",
                "total_signals": 0, "win_rate": 0.0, "avg_return": 0.0, "by_pattern": [],
            })
            continue
        if len(candles) < 20:
            items.append({
                "shcode": code, "name": name_for(code), "ok": False,
                "error": "캔들 부족(20개 미만)",
                "total_signals": 0, "win_rate": 0.0, "avg_return": 0.0, "by_pattern": [],
            })
            continue
        res = backtest_many(candles, patterns, tf, hold_bars, cost)
        res.update({"shcode": code, "name": name_for(code), "ok": True, "error": None})
        items.append(res)

    # 종목 가중(신호수) 집계
    graded = [it for it in items if it.get("ok") and it["total_signals"] > 0]
    total_signals = sum(it["total_signals"] for it in graded)
    if total_signals > 0:
        agg_avg = sum(it["avg_return"] * it["total_signals"] for it in graded) / total_signals
        agg_win = sum(it["win_rate"] * it["total_signals"] for it in graded) / total_signals
    else:
        agg_avg = 0.0
        agg_win = 0.0

    items.sort(key=lambda it: it.get("avg_return", 0), reverse=True)
    return {
        "timeframe": tf.value,
        "hold_bars": hold_bars,
        "strategy": strategy_name,
        "round_trip_cost_pct": cost.round_trip_cost() * 100,
        "count": len(items),
        "graded_count": len(graded),
        "aggregate": {
            "total_signals": total_signals,
            "avg_return": agg_avg,
            "win_rate": agg_win,
        },
        "items": items,
    }


@router.post("/compare-strategies")
async def compare_strategies(
    body: dict = Body(...),
    fetcher=Depends(get_fetcher),
    token: str = Depends(get_token),
):
    shcode = body["shcode"]
    tf = Timeframe(body.get("tf", "1d"))
    hold_bars = int(body.get("hold_bars", 5))
    candles = await fetcher.fetch(token, shcode, tf)

    rows: list = []
    for name, cfg in STRATEGY_PRESETS.items():
        ev = evaluate_strategy(candles, list(cfg.enabled_patterns), tf, hold_bars)
        agg = backtest_many(candles, list(cfg.enabled_patterns), tf, hold_bars, CostModel())
        rows.append({
            "preset": name,
            "entry_threshold": cfg.entry_threshold,
            "total_signals": agg["total_signals"],
            "win_rate": agg["win_rate"],
            "avg_return_net": agg["avg_return"],
            "oos_avg_return": ev["oos_avg_return"],
            "oos_consistency": ev["oos_consistency"],
        })
    rows.sort(key=lambda r: r["oos_avg_return"], reverse=True)
    return {"shcode": shcode, "timeframe": tf.value, "results": rows}
