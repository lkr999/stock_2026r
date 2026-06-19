"""OHLCV candle endpoints."""

from fastapi import APIRouter, Depends, HTTPException, Query

from app.dependencies import get_fetcher, get_token
from app.services.indicators import compute_all
from app.services.timeframe import Timeframe
from app.services.universe import name_for

router = APIRouter()


def _tf(tf: str) -> Timeframe:
    try:
        return Timeframe(tf)
    except ValueError:
        raise HTTPException(400, f"invalid timeframe: {tf}")


def _data_source(fetcher) -> str:
    # 모든 데이터는 eBest API 에서만 가져온다.
    return "live"


@router.get("/{shcode}")
async def get_candles(
    shcode: str,
    tf: str = Query("5m"),
    fetcher=Depends(get_fetcher),
    token: str = Depends(get_token),
):
    try:
        candles = await fetcher.fetch(token, shcode, _tf(tf))
    except Exception as exc:
        raise HTTPException(502, f"eBest 데이터 조회 오류: {exc}")
    return {
        "shcode": shcode,
        "name": name_for(shcode),
        "timeframe": tf,
        "candles": candles,
        "data_source": _data_source(fetcher),
    }


@router.get("/{shcode}/quote")
async def get_quote(
    shcode: str,
    tf: str = Query("1m"),
    fetcher=Depends(get_fetcher),
    token: str = Depends(get_token),
):
    """실시간 현재가: 해당 타임프레임 마지막 캔들 종가 + 직전 봉 대비 등락.

    현재가는 가장 최근 체결(마지막 캔들 종가)이며, 1분봉 기준이면 사실상 실시간이다.
    """
    try:
        candles = await fetcher.fetch(token, shcode, _tf(tf))
    except Exception as exc:
        raise HTTPException(502, f"eBest 데이터 조회 오류: {exc}")
    if not candles:
        raise HTTPException(404, "no candle data")
    last = candles[-1]
    prev = candles[-2] if len(candles) >= 2 else last
    price = float(last.get("close", 0) or 0)
    prev_close = float(prev.get("close", 0) or 0)
    change = price - prev_close
    change_pct = (change / prev_close * 100) if prev_close else 0.0
    return {
        "shcode": shcode,
        "name": name_for(shcode),
        "price": price,
        "prev_close": prev_close,
        "change": round(change, 2),
        "change_pct": round(change_pct, 2),
        "date": last.get("date", ""),
        "time": last.get("time", ""),
        "data_source": _data_source(fetcher),
    }


@router.get("/{shcode}/indicators")
async def get_candles_with_indicators(
    shcode: str,
    tf: str = Query("5m"),
    fetcher=Depends(get_fetcher),
    token: str = Depends(get_token),
):
    """캔들 + 보조지표(MA5/20/60, 볼린저, RSI14, MACD)를 함께 반환."""
    try:
        candles = await fetcher.fetch(token, shcode, _tf(tf))
    except Exception as exc:
        raise HTTPException(502, f"eBest 데이터 조회 오류: {exc}")
    return {
        "shcode": shcode,
        "name": name_for(shcode),
        "timeframe": tf,
        "candles": candles,
        "indicators": compute_all(candles),
        "data_source": _data_source(fetcher),
    }
