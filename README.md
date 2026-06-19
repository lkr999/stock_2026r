# Caginalp Candlestick Trader

Caginalp & Laurent (1998) 8-pattern candlestick trading system for the Korean
market (KOSPI/KOSDAQ), implementing the spec in
[CANDLESTICK_TRADING_SYSTEM.md](CANDLESTICK_TRADING_SYSTEM.md).

**Stack:** FastAPI (backend) + SvelteKit (frontend) + eBest REST API.

## What's implemented (Phase 1–2 MVP)

- ✅ **8 rule-based reversal patterns** (Three White Soldiers, Morning/Evening Star,
  Hammer, Hanging Man, Bullish/Bearish Engulfing, Three Black Crows)
- ✅ **Multi-timeframe** data (1m–1d) via eBest t8412/t8410
- ✅ **Modern overlays:** ATR-adaptive threshold, volume confirmation, MTF confluence
- ✅ **Strategy mix/select** — tunable `StrategyConfig` weights + 4 presets
  (conservative / balanced / aggressive / ml_blended)
- ✅ **Cost-aware backtest** (fee + tax + slippage) with profit_factor, MDD, Sharpe
- ✅ **Walk-forward** out-of-sample evaluation + strategy comparison
- ✅ **Automated trading engine** — paper/live parity, ATR position sizing,
  stop-loss / take-profit / trailing stop, daily-loss-limit gate
- ✅ **eBest-only data** — 모든 시세/캔들은 eBest API 에서만 조회합니다.
  합성/모의 데이터는 생성하지 않으며, 키 미설정 시 데이터 조회는 인증 오류를 반환합니다.

Deferred (Phase 4): ML ensemble (`scikit-learn`) and GAF-CNN (`torch`) — hooks
exist (`ml_score`, `gaf_score`) but require training data + walk-forward validation
before live use.

## Quick start

### 1. Backend

```bash
cd backend
python3.13 -m venv .venv      # Python 3.13+ required
.venv/bin/pip install -r requirements.txt
cp .env.example .env          # EBEST_APP_KEY / EBEST_APP_SECRET 필수 (실데이터 전용)

# 모든 데이터는 eBest API 에서만 가져옵니다 (.env 의 EBEST_* 키 필요):
.venv/bin/python -m uvicorn app.main:app --port 8000
```

Docs at http://localhost:8000/docs

### 2. Frontend

```bash
cd frontend
npm install
npm run dev        # http://localhost:5173  (proxies /api -> :8000)
```

## Pages

| Route | Purpose |
|-------|---------|
| `/` | Signal dashboard — universe scan, timeframe/strategy/market filters, 30s refresh |
| `/chart/[code]` | Candle chart + pattern markers + composite-score breakdown |
| `/trading` | Automated trading control — paper/live, live weight sliders, positions, daily P&L |
| `/backtest` | Cost-aware backtest + walk-forward strategy preset comparison |

## Tests

```bash
cd backend && .venv/bin/python -m pytest -q
```

Covers pattern detection, risk sizing/stops/limits, and cost-model arithmetic.

## 모의투자 / 실전투자 모드 선택

You **freely select** the mode on the `/trading` page — 모의투자(paper) or 실전투자(live).
Neither is forced. A **readiness report is shown as advisory only** (it does not block
live trading), so you can decide when to go live.

Advisory criteria (recommended before live, spec section 11-2):

| Criterion | Default | Env var |
|-----------|---------|---------|
| Paper trades | ≥ 30 | `LIVE_MIN_PAPER_TRADES` |
| Validation span | ≥ 14 days | `LIVE_MIN_PAPER_DAYS` |
| Win rate (net) | ≥ 45% | `LIVE_MIN_WIN_RATE` |
| Profit factor | ≥ 1.3 | `LIVE_MIN_PROFIT_FACTOR` |
| Cumulative P&L | > 0 | `LIVE_REQUIRE_POSITIVE_PNL` |

How it works:
1. Every closed trade is journaled to `backend/data/trade_journal.json`.
2. `GET /api/trading/readiness` reports per-criterion pass/fail, shown on the
   `/trading` page as the **실전 전환 검증 (참고)** panel.
3. `POST /api/trading/start` accepts either mode and **always** starts; if you start
   `live` while criteria are unmet, the response includes a `readiness_advisory`
   warning (the UI also shows a confirm dialog). It never blocks.

`TRADING_MODE=paper` is the default. Live orders still pass the per-trade
`RiskManager` gate (sizing, stop-loss, daily-loss limit) regardless of mode.

## Requirements

Python **3.13+** (uses PEP 695 `type` aliases, PEP 604 unions). Node 18+.

## Architecture

```
backend/app/
  services/
    timeframe.py        Timeframe enum + per-tf config + MTF groups
    candle_fetcher.py   t8412/t8410 -> unified OHLCV (eBest 전용)
    universe.py         KOSPI/KOSDAQ list from files/search_item.csv
    pattern_detector.py 8 patterns + ATR/volume + strategy-based composite
    strategy.py         StrategyConfig (mix/select) + presets
    risk.py             RiskManager (sizing, stops, daily limit)
    broker.py           paper/live order execution (CSPAT00601)
    trading_engine.py   polling loop: exits-first then gated entries
    mtf_engine.py       multi-timeframe confluence score
    backtest_engine.py  CostModel + run_backtest + walk-forward
  routers/              candles, patterns, signals, backtest, trading
  main.py               app wiring + lifespan
```
# stock_2006
