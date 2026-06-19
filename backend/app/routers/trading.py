"""Automated trading control endpoints (spec section 10-5)."""

from fastapi import APIRouter, Body, HTTPException, Query, Request

from app.config import Settings, get_settings
from app.services.broker import Broker, TradingMode
from app.services.risk import RiskConfig, RiskManager
from app.services.strategy import STRATEGY_PRESETS, resolve_strategy
from app.services.trading_engine import OrderConfig, TradingEngine
from app.services.timeframe import Timeframe
from app.services.validation import ReadinessCriteria, evaluate_readiness

router = APIRouter()


def _criteria(settings: Settings) -> ReadinessCriteria:
    return ReadinessCriteria(
        min_paper_trades=settings.LIVE_MIN_PAPER_TRADES,
        min_paper_days=settings.LIVE_MIN_PAPER_DAYS,
        min_win_rate=settings.LIVE_MIN_WIN_RATE,
        min_profit_factor=settings.LIVE_MIN_PROFIT_FACTOR,
        require_positive_pnl=settings.LIVE_REQUIRE_POSITIVE_PNL,
    )


@router.get("/readiness")
async def readiness(request: Request):
    """Paper-trading track record vs the live-trading gate criteria."""
    settings = get_settings()
    report = evaluate_readiness(request.app.state.journal, _criteria(settings))
    return {
        **report.to_dict(),
        "force_allowed": settings.LIVE_ALLOW_FORCE,
    }


@router.get("/journal")
async def journal(request: Request, mode: str = Query("paper"), limit: int = Query(100)):
    trades = request.app.state.journal.by_mode(mode)
    return trades[-limit:][::-1]


@router.get("/stats")
async def trade_stats(request: Request, mode: str = Query("all")):
    """집계 통계: 총 손익, 승률, 손익비 등."""
    j = request.app.state.journal
    return j.stats(None if mode == "all" else mode)


@router.post("/positions/{code}/close")
async def close_position(request: Request, code: str):
    """보유 포지션을 종목별로 수동 청산(전량 매도)한다."""
    engine = getattr(request.app.state, "engine", None)
    if not engine:
        raise HTTPException(404, "no engine")
    res = await engine.manual_close(code)
    if not res.get("ok"):
        reason = res.get("reason", "close_failed")
        code_map = {"no_position": 404}
        raise HTTPException(code_map.get(reason, 400), reason)
    return {"ok": True, **res, "status": engine.status()}


@router.post("/events/clear")
async def clear_events(request: Request):
    """이번 세션 거래 기록(엔진 인메모리 + 차트 마커)을 삭제한다."""
    engine = getattr(request.app.state, "engine", None)
    removed = engine.clear_events() if engine else 0
    status = engine.status() if engine else {"running": False, "positions": [], "trade_events": []}
    return {"ok": True, "removed": removed, "status": status}


@router.post("/journal/clear")
async def clear_journal(request: Request, mode: str = Query("all")):
    """누적 거래내역(영구 저장)을 삭제한다. mode=all|paper|live."""
    j = request.app.state.journal
    if mode == "all":
        before = len(j.all())
        j.clear()
        removed = before
    else:
        removed = j.clear_mode(mode)
    return {"ok": True, "removed": removed}


@router.get("/presets")
async def presets():
    out = {}
    for name, cfg in STRATEGY_PRESETS.items():
        out[name] = {
            "name": cfg.name,
            "weights": {s.value: w for s, w in cfg.weights.items()},
            "enabled_patterns": list(cfg.enabled_patterns),
            "entry_threshold": cfg.entry_threshold,
            "direction": cfg.direction,
        }
    return out


@router.post("/start")
async def start(request: Request, body: dict = Body(...)):
    app = request.app
    if getattr(app.state, "engine", None) and app.state.engine.running:
        raise HTTPException(409, "trading engine already running")

    settings = get_settings()
    mode = TradingMode(body.get("mode", settings.TRADING_MODE))

    # 모드는 사용자가 자유롭게 선택한다. 검증 리포트는 참고용으로만 함께 반환한다.
    readiness_report = evaluate_readiness(app.state.journal, _criteria(settings))

    strategy = resolve_strategy(body.get("strategy", settings.TRADING_DEFAULT_STRATEGY))
    watchlist = body.get("watchlist") or []
    if not watchlist:
        raise HTTPException(400, "watchlist required")
    tf = Timeframe(body.get("tf", "5m"))
    poll_sec = int(body.get("poll_sec", 60))

    rc = body.get("risk") or {}
    risk_cfg = RiskConfig(
        max_position_pct=float(rc.get("max_position_pct", 0.10)),
        max_positions=int(rc.get("max_positions", settings.TRADING_MAX_POSITIONS)),
        risk_per_trade_pct=float(rc.get("risk_per_trade_pct", settings.TRADING_RISK_PER_TRADE)),
        stop_loss_atr_mult=float(rc.get("stop_loss_atr_mult", 1.5)),
        take_profit_atr_mult=float(rc.get("take_profit_atr_mult", 3.0)),
        daily_loss_limit_pct=float(rc.get("daily_loss_limit_pct", settings.TRADING_DAILY_LOSS_LIMIT)),
        trailing_stop_atr=float(rc.get("trailing_stop_atr", 2.0)),
    )

    oc = body.get("order") or {}
    order_cfg = OrderConfig(
        order_type=oc.get("order_type", settings.TRADING_ORDER_TYPE),
        fixed_qty=int(oc.get("fixed_qty") or 0) or None,
        sell_all=bool(oc.get("sell_all", settings.TRADING_SELL_ALL)),
        max_buy_amount=float(oc.get("max_buy_amount", settings.TRADING_MAX_BUY_AMOUNT)),
    )

    broker = Broker(app.state.ebest, mode=mode)
    engine = TradingEngine(
        ebest=app.state.ebest,
        fetcher=app.state.fetcher,
        detector=app.state.detector,
        broker=broker,
        strategy=strategy,
        risk=RiskManager(risk_cfg),
        watchlist=watchlist,
        tf=tf,
        ignore_market_hours=bool(body.get("ignore_market_hours", False)),
        journal=app.state.journal,
        seed_cash=settings.TRADING_PAPER_SEED,
        order=order_cfg,
    )
    engine.start(poll_sec=poll_sec)
    app.state.engine = engine
    return {
        "ok": True,
        "mode": mode.value,
        "status": engine.status(),
        # 참고용: 실전 모드인데 검증 미달이면 경고만 표시 (차단하지 않음)
        "readiness_advisory": (
            None if (mode != TradingMode.LIVE or readiness_report.ready)
            else {
                "recommended": False,
                "message": "검증 기준 미달 상태에서 실전투자를 시작했습니다. 권장: 모의투자로 충분히 검증하세요.",
                "failed_criteria": [c.label for c in readiness_report.criteria if not c.passed],
            }
        ),
    }


@router.post("/stop")
async def stop(request: Request):
    engine = getattr(request.app.state, "engine", None)
    if not engine:
        raise HTTPException(404, "no engine")
    engine.stop()
    return {"ok": True, "status": engine.status()}


@router.get("/status")
async def status(request: Request):
    engine = getattr(request.app.state, "engine", None)
    if not engine:
        return {"running": False, "positions": [], "daily_pnl": 0.0}
    return engine.status()


@router.put("/strategy")
async def update_strategy(request: Request, body: dict = Body(...)):
    engine = getattr(request.app.state, "engine", None)
    if not engine:
        raise HTTPException(404, "no engine")
    engine.strategy = resolve_strategy(body)
    return {"ok": True, "strategy": engine.strategy.name}
