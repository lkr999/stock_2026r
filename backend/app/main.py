"""FastAPI entrypoint for the Caginalp candlestick trading system."""

import logging
from contextlib import asynccontextmanager

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from app.config import get_settings
from app.routers import backtest, candles, patterns, signals, trading
from app.services.candle_fetcher import CandleFetcher
from app.services.ebest_service import EBestService, _parse_ebest_float
from app.services.journal import TradeJournal
from app.services.pattern_detector import PatternDetector
from app.services.timeframe import Timeframe
from app.services.universe import filter_universe, info_for, load_universe

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
logger = logging.getLogger(__name__)


@asynccontextmanager
async def lifespan(app: FastAPI):
    settings = get_settings()

    # 모든 데이터는 오직 eBest API를 통해서만 가져온다 (합성/모의 데이터 없음).
    ebest = EBestService()
    await ebest.start()
    app.state.ebest = ebest
    app.state.fetcher = CandleFetcher(ebest)
    app.state.ebest_configured = bool(settings.EBEST_APP_KEY and settings.EBEST_APP_SECRET)
    if not app.state.ebest_configured:
        logger.warning(
            "eBest 키 미설정 — .env 의 EBEST_APP_KEY/EBEST_APP_SECRET 를 설정하세요. "
            "설정 전에는 데이터 조회 시 인증 오류가 반환됩니다 (가짜 데이터는 생성하지 않습니다)."
        )

    app.state.detector = PatternDetector()
    app.state.journal = TradeJournal()
    app.state.engine = None
    yield

    engine = getattr(app.state, "engine", None)
    if engine:
        engine.stop()
    await app.state.ebest.aclose()


settings = get_settings()
app = FastAPI(title="Caginalp Candlestick Trader", version="1.0.0", lifespan=lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=[o.strip() for o in settings.CORS_ORIGINS.split(",")] + ["http://localhost:5173"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

app.include_router(candles.router, prefix="/api/candles", tags=["candles"])
app.include_router(patterns.router, prefix="/api/patterns", tags=["patterns"])
app.include_router(signals.router, prefix="/api/signals", tags=["signals"])
app.include_router(backtest.router, prefix="/api/backtest", tags=["backtest"])
app.include_router(trading.router, prefix="/api/trading", tags=["trading"])


@app.get("/api/health")
async def health():
    return {"ok": True, "ebest_configured": app.state.ebest_configured}


@app.get("/api/ebest/status")
async def ebest_status():
    """eBest API 통신 상태: 키 설정 여부, 토큰 발급 가능 여부."""
    settings = get_settings()
    has_keys = bool(settings.EBEST_APP_KEY and settings.EBEST_APP_SECRET)
    token_ok = False
    try:
        token_ok = bool(await app.state.ebest.auth_token())
    except Exception:
        token_ok = False
    return {
        "has_keys": has_keys,
        "token_ok": token_ok,
        "base_url": settings.EBEST_URL,
    }


@app.post("/api/ebest/test")
async def ebest_test(code: str = "005930"):
    """예제 API 통신 테스트: 토큰 발급 → 분봉/일봉/현재가를 단계별로 점검.

    모든 데이터는 eBest API 실거래 서버에서만 가져온다 (합성 데이터 없음).
    현재가는 t1101 스냅샷 1콜로 조회해 분봉 500봉 조회 대비 빠르고,
    분봉(t8452)과 TR 코드가 달라 레이트리밋 대기도 발생하지 않는다.
    """
    import time as _time

    ebest = app.state.ebest
    fetcher = app.state.fetcher
    checks: list[dict] = []

    async def run(name: str, tr: str, coro):
        t0 = _time.perf_counter()
        try:
            value = await coro()
            ms = round((_time.perf_counter() - t0) * 1000, 1)
            return {"name": name, "tr": tr, "ok": True, "latency_ms": ms, "detail": value}
        except Exception as exc:
            ms = round((_time.perf_counter() - t0) * 1000, 1)
            return {"name": name, "tr": tr, "ok": False, "latency_ms": ms, "detail": str(exc)}

    # 1) 인증 토큰 — 한 번만 발급하고 이후 단계에서 재사용한다.
    token = ""

    async def _token():
        nonlocal token
        token = await ebest.auth_token() or ""
        if not token:
            raise RuntimeError("토큰 발급 실패 — eBest 키/네트워크 확인 필요")
        return f"발급 성공 (…{token[-6:]})"

    checks.append(await run("인증 토큰 발급", "oauth2/token", _token))

    # 2) 분봉 조회 (t8452)
    async def _min():
        rows = await fetcher.fetch(token, code, Timeframe.M5)
        if not rows:
            raise RuntimeError("응답 없음")
        last = rows[-1]
        return f"{len(rows)}봉 · 최근 종가 {last['close']:.0f} ({last.get('date','')} {last.get('time','')})"

    checks.append(await run("분봉 조회 (t8452 · 5분)", "t8452", _min))

    # 3) 일봉 조회 (t8451)
    async def _day():
        rows = await fetcher.fetch(token, code, Timeframe.D1)
        if not rows:
            raise RuntimeError("응답 없음")
        return f"{len(rows)}봉 · 최근 종가 {rows[-1]['close']:.0f} ({rows[-1].get('date','')})"

    checks.append(await run("일봉 조회 (t8451)", "t8451", _day))

    # 4) 현재가 조회 (t1101 스냅샷)
    async def _quote():
        snap = await ebest.stock_price(token, code)
        if not snap or snap.get("_rate_limited"):
            raise RuntimeError("응답 없음")
        price = _parse_ebest_float(snap.get("price")) or 0.0
        if price <= 0:
            raise RuntimeError("현재가 없음")
        return f"현재가 {price:.0f}원 (등락 {snap.get('drate', 0.0):.2f}%)"

    checks.append(await run("현재가 조회 (t1101)", "t1101", _quote))

    ok_all = all(c["ok"] for c in checks)
    return {
        "ok": ok_all,
        "source": "ebest",
        "code": code,
        "name": info_for(code).get("name", ""),
        "checks": checks,
    }


@app.get("/api/universe")
async def universe(
    market: str = "ALL",
    min_price: float | None = None,
    max_price: float | None = None,
    q: str | None = None,
    limit: int = 200,
):
    rows = filter_universe(market=market, min_price=min_price, max_price=max_price)
    if q:
        ql = q.strip().lower()
        rows = [r for r in rows if ql in r["name"].lower() or ql in r["code"]]
    return {"total": len(rows), "items": rows[:limit]}


@app.get("/api/universe/lookup")
async def lookup(codes: str = ""):
    """code,name,market,price 조회. codes=콤마구분 종목코드."""
    out = []
    for code in [c.strip() for c in codes.split(",") if c.strip()]:
        info = info_for(code)
        out.append({
            "code": code,
            "name": info.get("name", ""),
            "market": info.get("market", ""),
            "price": info.get("price", 0),
        })
    return out


@app.get("/api/universe/price-range")
async def price_range(market: str = "ALL"):
    rows = filter_universe(market=market)
    prices = [r["price"] for r in rows if r["price"] > 0]
    return {
        "count": len(rows),
        "min": min(prices) if prices else 0,
        "max": max(prices) if prices else 0,
    }
