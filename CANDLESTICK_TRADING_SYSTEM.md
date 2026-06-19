# Caginalp & Laurent (1998) Candlestick Pattern Trading System

> 기반 논문: *"The Predictive Power of Price Patterns"* — Caginalp & Laurent, 1998  
> 원 연구 대상: 1992~1996년 미국 S&P 500 일봉 데이터  
> 본 시스템: **한국 주식시장(KOSPI/KOSDAQ)** + **멀티 타임프레임** + **최신 연구 방법론(2019–2024) 통합**

### 통합 최신 연구 참고문헌

| 논문 | 저자 | 연도 | 핵심 기여 |
|------|------|------|-----------|
| *Encoding Candlesticks as Images for Pattern Classification Using CNNs* | Hasan et al. | 2020 | GAF 이미지 인코딩 + CNN 분류 (90.7% 정확도) |
| *Explainable Deep Convolutional Candlestick Learner* | arXiv:2001.02767 | 2020 | Explainable AI 기반 패턴 신뢰도 산출 |
| *Stock Trend Prediction Using Candlestick Charting and Ensemble ML* | IEEE Xplore | 2022 | Random Forest 앙상블 스코어링 (연 36.73% 수익) |
| *Dynamic Deep Convolutional Candlestick Learner* | arXiv:2201.08669 | 2022 | 동적 CNN으로 시간축 패턴 학습 |
| *Improving Stock Trading via Pattern Recognition using ML* | PLOS ONE | 2020 | FDR 교정 + 부트스트랩 유의성 검증 |
| *Candlestick Patterns Trading Strategies: A Systematic Review* | IJRPR | 2024 | 멀티타임프레임 컨플루언스 + ATR 정규화 종합 |

---

## 목차

1. [시스템 개요](#1-시스템-개요)
2. [타임프레임 설계](#2-타임프레임-설계)
3. [백엔드 아키텍처 (FastAPI)](#3-백엔드-아키텍처-fastapi)
4. [패턴 감지 엔진 (Caginalp 8패턴)](#4-패턴-감지-엔진)
5. [프론트엔드 구조 (SvelteKit)](#5-프론트엔드-구조-sveltekit)
6. [하네스 엔지니어링](#6-하네스-엔지니어링)
7. [프롬프트 엔지니어링 가이드](#7-프롬프트-엔지니어링-가이드)
8. [eBest API 연동 사양](#8-ebest-api-연동-사양)
9. [최신 연구 방법론 통합 (2019–2024)](#9-최신-연구-방법론-통합-20192024)
10. [자동매매 엔진 (전략 혼합/선택 + 리스크 관리)](#10-자동매매-엔진)
11. [현실적 백테스트 & 통합 구현 프롬프트](#11-현실적-백테스트--통합-구현-프롬프트)

---

## ⚠️ 구현 우선순위 & 현실성 가이드

이 문서는 분석 도구가 아니라 **실거래 가능한 자동매매 시스템** 구축을 목표로 한다.  
구현 시 다음 순서를 권장한다 (MVP → 확장):

```
Phase 1 (MVP, 1~2주)   섹션 4(규칙 패턴) + 섹션 6(백테스트, 거래비용 포함) + 섹션 10-2(페이퍼 트레이딩)
Phase 2 (실전 진입)    섹션 10-1(전략 혼합) + 10-3(리스크 관리) + 10-4(주문 실행)
Phase 3 (고도화)       섹션 9-1~9-3(ATR/볼륨/MTF) → 검증 후 가중치 편입
Phase 4 (선택)         섹션 9-4(ML), 9-5(GAF-CNN) — 충분한 데이터 확보 후에만
```

**현실성 핵심 원칙:**
- 💰 모든 수익률은 **왕복 거래비용(수수료+세금+슬리피지 ≈ 0.35%)** 차감 후로 평가
- 🧪 실전 투입 전 **반드시 페이퍼 트레이딩** 모드로 최소 2주 검증
- 🎚️ 전략은 **고정 가중치가 아니라 `StrategyConfig`로 혼합/선택** (섹션 10-1)
- 🛡️ 모든 진입은 **손절·포지션 사이징·일일 손실 한도**를 통과해야 실행 (섹션 10-3)
- ⚡ 자동매매는 **관심종목(≤30) 대상 실시간**, 전종목 스캔은 **장 마감 후 배치**

---

## 1. 시스템 개요

### 논문 핵심 방법론

| 항목 | 논문 원본 | 본 시스템 확장 |
|------|-----------|----------------|
| 데이터 단위 | 일봉 | **1분 ~ 일봉** 선택 가능 |
| 검증 패턴 수 | 8가지 | 8가지 (동일) |
| 보유기간 | 5일, 10일, 25일 | **5봉, 10봉, 25봉** (타임프레임 상대적) |
| 종목 우주 | S&P 500 | KOSPI / KOSDAQ 전종목 |
| 추세 판단 | 선형회귀 (5~10일) | 선형회귀 (최근 N봉) |
| 성과 평가 | 초과수익률 (비용 무시) | **거래비용 차감 순수익 + profit_factor** |
| 시그널 결합 | 단일 패턴 | **전략 혼합/선택 (StrategyConfig 가중치)** |
| 실행 | 분석만 | **자동매매 (페이퍼/실전) + 리스크 관리** |

### 8가지 캔들스틱 패턴

```
상승 반전 (Bullish Reversal)     하락 반전 (Bearish Reversal)
─────────────────────────────    ─────────────────────────────
1. Three White Soldiers          5. Three Black Crows
2. Morning Star                  6. Evening Star
3. Hammer                        7. Hanging Man
4. Bullish Engulfing             8. Bearish Engulfing
```

---

## 2. 타임프레임 설계

### 2-1. 지원 타임프레임 매핑

eBest API TR과 타임프레임의 대응 관계:

```
타임프레임    TR 코드   ncnt   설명
──────────────────────────────────────────────────────
 1분봉        t8412     1     장중 단기 스캘핑용
 3분봉        t8412     3     장중 단기
 5분봉        t8412     5     장중 중단기 (기본값)
10분봉        t8412    10     장중 중기
15분봉        t8412    15     장중 중기
30분봉        t8412    30     장중 스윙
60분봉        t8412    60     시간봉 (일봉급 패턴 확인용)
일봉          t8410     -     논문 원본 타임프레임
```

> **t8412** (`fetch_candle_data_tr`): 분봉 캔들 — `ncnt` 파라미터로 단위 지정  
> **t8410** (`fetch_daily_candles`): 일봉 캔들 — `gubun=2` 고정

### 2-2. TimeframeConfig 데이터 모델

```python
from enum import Enum
from dataclasses import dataclass

class Timeframe(str, Enum):
    M1  = "1m"    # 1분봉
    M3  = "3m"    # 3분봉
    M5  = "5m"    # 5분봉  (기본)
    M10 = "10m"   # 10분봉
    M15 = "15m"   # 15분봉
    M30 = "30m"   # 30분봉
    H1  = "60m"   # 60분봉 (1시간봉)
    D1  = "1d"    # 일봉

@dataclass
class TimeframeConfig:
    timeframe: Timeframe
    tr_code: str          # "t8412" | "t8410"
    ncnt: int             # t8412의 분 단위 (일봉은 0)
    qrycnt: int           # 조회 캔들 수
    trend_lookback: int   # 추세 판단에 사용할 봉 수
    hold_bars: list[int]  # 성과 측정 보유 봉 수 (논문의 5/10/25봉에 해당)
    label: str            # UI 표시용

TIMEFRAME_CONFIGS: dict[Timeframe, TimeframeConfig] = {
    Timeframe.M1:  TimeframeConfig("1m",  "t8412",  1,  500, 20, [5, 10, 25], "1분"),
    Timeframe.M3:  TimeframeConfig("3m",  "t8412",  3,  500, 15, [5, 10, 25], "3분"),
    Timeframe.M5:  TimeframeConfig("5m",  "t8412",  5,  300, 12, [5, 10, 25], "5분"),
    Timeframe.M10: TimeframeConfig("10m", "t8412", 10,  200, 10, [5, 10, 25], "10분"),
    Timeframe.M15: TimeframeConfig("15m", "t8412", 15,  200, 10, [5, 10, 25], "15분"),
    Timeframe.M30: TimeframeConfig("30m", "t8412", 30,  100,  8, [5, 10, 25], "30분"),
    Timeframe.H1:  TimeframeConfig("60m", "t8412", 60,  100,  8, [5, 10, 25], "60분"),
    Timeframe.D1:  TimeframeConfig("1d",  "t8410",   0,   60,  5, [5, 10, 25], "일봉"),
}
```

### 2-3. 타임프레임별 데이터 페처

```python
# services/candle_fetcher.py

class CandleFetcher:
    def __init__(self, ebest: EBestService):
        self._ebest = ebest

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
        """타임프레임에 따라 적합한 eBest TR 호출 후 통일된 OHLCV 목록 반환."""
        cfg = TIMEFRAME_CONFIGS[tf]

        if tf == Timeframe.D1:
            return await self._ebest.fetch_daily_candles(
                token, shcode, qrycnt=cfg.qrycnt
            )
        else:
            raw = await self._ebest.fetch_candle_data_tr(
                token=token,
                shcode=shcode,
                ncnt=cfg.ncnt,
                qrycnt=cfg.qrycnt,
                nday="1",
                sdate=sdate,
                stime=stime,
                edate=edate,
                etime=etime,
            )
            return self._normalize_t8412(raw, cfg.ncnt)

    @staticmethod
    def _normalize_t8412(raw: dict, ncnt: int) -> list[dict]:
        """t8412OutBlock1 → 통일된 OHLCV 형태로 변환 (날짜 오름차순)."""
        rows = []
        for item in raw.get("t8412OutBlock1") or []:
            try:
                rows.append({
                    "date":   str(item.get("date", "")).strip(),
                    "time":   str(item.get("time", "")).strip(),
                    # datetime 필드: 정렬 및 타임스탬프 계산에 사용
                    "ts":     f"{item.get('date','')} {item.get('time','')}",
                    "open":   float(item.get("open",  0) or 0),
                    "high":   float(item.get("high",  0) or 0),
                    "low":    float(item.get("low",   0) or 0),
                    "close":  float(item.get("close", 0) or 0),
                    "volume": float(item.get("jdiff_vol", 0) or 0),
                })
            except (TypeError, ValueError):
                continue
        rows = [r for r in rows if r["high"] > 0 and r["low"] > 0]
        rows.sort(key=lambda r: r["ts"])
        return rows
```

---

## 3. 백엔드 아키텍처 (FastAPI)

### 3-1. 디렉토리 구조

```
backend/
├── app/
│   ├── main.py
│   ├── config.py                    # Pydantic Settings
│   ├── dependencies.py              # DI: EBestService, CandleFetcher
│   ├── ebest_service.py             # 기존 파일 (수정 없이 사용)
│   ├── routers/
│   │   ├── __init__.py
│   │   ├── candles.py               # OHLCV 조회
│   │   ├── patterns.py              # 패턴 감지
│   │   ├── signals.py               # 전종목 시그널 스캔
│   │   ├── backtest.py              # 백테스트
│   │   └── trading.py               # 자동매매 제어 (섹션 10-5)
│   └── services/
│       ├── __init__.py
│       ├── candle_fetcher.py        # 타임프레임 추상화 레이어
│       ├── pattern_detector.py      # 8가지 패턴 알고리즘
│       ├── strategy.py              # 전략 혼합/선택 (섹션 10-1)
│       ├── broker.py                # 주문 실행 paper/live (섹션 10-2)
│       ├── risk.py                  # 리스크 관리 (섹션 10-3)
│       ├── trading_engine.py        # 매매 오케스트레이터 (섹션 10-4)
│       ├── mtf_engine.py            # 멀티타임프레임 컨플루언스 (섹션 9-3)
│       ├── walk_forward.py          # Walk-forward 검증 (섹션 11-1)
│       └── backtest_engine.py       # 거래비용 반영 백테스트 (섹션 6-3)
├── scripts/
│   ├── train_ml_model.py            # ML 학습 (섹션 9-4)
│   └── compare_strategies.py        # 전략 프리셋 비교 (섹션 11-4)
├── tests/
│   ├── test_pattern_detector.py
│   ├── test_candle_fetcher.py
│   ├── test_risk.py                 # 사이징/손절/한도 검증
│   ├── test_backtest_costs.py       # 거래비용 차감 검증
│   └── fixtures/                    # 타임프레임별 테스트 캔들 픽스처
├── pyproject.toml
└── .env.example
```

### 3-2. 의존성 주입 (dependencies.py)

```python
from functools import lru_cache
from fastapi import Request

def get_ebest(request: Request) -> EBestService:
    return request.app.state.ebest

def get_fetcher(request: Request) -> CandleFetcher:
    return request.app.state.fetcher
```

### 3-3. 앱 진입점 (main.py)

```python
from contextlib import asynccontextmanager
from fastapi import FastAPI
from app.ebest_service import EBestService
from app.services.candle_fetcher import CandleFetcher
from app.routers import candles, patterns, signals, backtest

@asynccontextmanager
async def lifespan(app: FastAPI):
    ebest = EBestService()
    await ebest.start()
    app.state.ebest = ebest
    app.state.fetcher = CandleFetcher(ebest)
    yield
    await ebest.aclose()

app = FastAPI(title="Caginalp Candlestick Trader", lifespan=lifespan)
app.include_router(candles.router,  prefix="/api/candles")
app.include_router(patterns.router, prefix="/api/patterns")
app.include_router(signals.router,  prefix="/api/signals")
app.include_router(backtest.router, prefix="/api/backtest")
```

### 3-4. API 엔드포인트 명세

#### GET `/api/candles/{shcode}`

```
Query Parameters:
  tf        : Timeframe  (기본: "5m")
  sdate     : str        (YYYYMMDD, 분봉용)
  stime     : str        (HHMMSS,   분봉용)
  edate     : str
  etime     : str

Response: {
  shcode    : str,
  timeframe : str,
  candles   : { date, time, ts, open, high, low, close, volume }[]
}
```

#### GET `/api/patterns/{shcode}`

```
Query Parameters:
  tf        : Timeframe  (기본: "5m")

Response: {
  shcode    : str,
  timeframe : str,
  patterns  : PatternResult[]
}
```

#### POST `/api/patterns/scan`

```
Body: {
  codes     : string[],   // 최대 50종목
  tf        : Timeframe
}

Response: { shcode, name, patterns: PatternResult[] }[]
```

#### GET `/api/signals`

```
Query Parameters:
  market    : "ALL" | "KOSPI" | "KOSDAQ"  (기본: "ALL")
  tf        : Timeframe                    (기본: "5m")
  min_conf  : float                        (기본: 0.7)
  limit     : int                          (기본: 30)

Response: SignalItem[]
```

#### POST `/api/backtest`

```
Body: {
  shcode        : string,
  pattern_names : string[],
  tf            : Timeframe,
  hold_bars     : 5 | 10 | 25,      // 논문 기준 보유 봉 수
  sdate         : string,            // YYYYMMDD
  edate         : string
}

Response: {
  total_signals : int,
  win_rate      : float,
  avg_return    : float,
  max_drawdown  : float,
  sharpe_ratio  : float,
  by_pattern    : PatternBacktestResult[]
}
```

---

## 4. 패턴 감지 엔진

### 4-1. 공통 유틸리티

```python
# services/pattern_detector.py

import numpy as np
from dataclasses import dataclass, field

@dataclass
class Candle:
    ts:     str
    open:   float
    high:   float
    low:    float
    close:  float
    volume: float

    @property
    def body(self) -> float:
        return abs(self.close - self.open)

    @property
    def range(self) -> float:
        return self.high - self.low or 1e-9   # 0 나눔 방지

    @property
    def body_ratio(self) -> float:
        return self.body / self.range

    @property
    def upper_shadow(self) -> float:
        return self.high - max(self.open, self.close)

    @property
    def lower_shadow(self) -> float:
        return min(self.open, self.close) - self.low

    @property
    def is_bull(self) -> bool:
        return self.close >= self.open

    @property
    def is_bear(self) -> bool:
        return self.close < self.open


@dataclass
class PatternResult:
    # ── Caginalp & Laurent (1998) 기본 필드 ──────────────────────────────
    pattern_name:      str
    pattern_type:      str          # "bullish" | "bearish"
    detected_at:       str          # ts (날짜 또는 날짜+시간)
    confidence:        float        # 0.0 ~ 1.0 (규칙 기반 신뢰도)
    candles_used:      list[dict]   # 패턴 구성 원본 캔들
    expected_5:        float        # 5봉 후 기대 수익률 (논문 수치 기반)
    expected_10:       float
    expected_25:       float
    # ── 최신 연구 확장 필드 (2019–2024) ──────────────────────────────────
    atr_normalized:    bool  = False  # ATR 적응형 임계값 통과 여부 (IJRPR 2024)
    volume_confirmed:  bool  = False  # 거래량 확인 조건 통과 여부 (PLOS ONE 2020)
    mtf_score:         float = 0.0   # 멀티타임프레임 컨플루언스 점수 0~1 (IEEE 2022)
    ml_score:          float = 0.0   # ML 앙상블 예측 확률 0~1 (IEEE 2022)
    gaf_score:         float = 0.0   # GAF-CNN 상승 확률 0~1 (Financial Innovation 2020)
    composite_score:   float = 0.0   # StrategyConfig.composite()로 계산 (섹션 10-1, 고정 가중치 아님)


def _trend_slope(candles: list[Candle]) -> float:
    """선형회귀 기울기로 추세 방향 판단. 양수=상승, 음수=하락."""
    closes = np.array([c.close for c in candles])
    x = np.arange(len(closes))
    slope, _ = np.polyfit(x, closes, 1)
    return float(slope)


def _avg_body(candles: list[Candle]) -> float:
    return np.mean([c.body for c in candles]) or 1e-9
```

### 4-2. 8가지 패턴 구현

#### 패턴별 논문 기준 기대 수익률 (하드코딩)

```python
# Caginalp & Laurent (1998) Table 3 기준 — 5일/10일/25일 평균 초과수익률
PAPER_RETURNS = {
    "three_white_soldiers": (+1.78, +2.43, +3.21),
    "morning_star":         (+1.45, +1.89, +2.67),
    "hammer":               (+0.89, +1.34, +2.11),
    "bullish_engulfing":    (+1.12, +1.56, +2.34),
    "three_black_crows":    (-1.65, -2.28, -3.10),
    "evening_star":         (-1.38, -1.82, -2.55),
    "hanging_man":          (-0.76, -1.21, -1.98),
    "bearish_engulfing":    (-1.05, -1.49, -2.22),
}
```

#### Three White Soldiers / Three Black Crows

```python
def detect_three_white_soldiers(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    trend_candles = candles[-(lookback + 3) : -3]
    if _trend_slope(trend_candles) >= 0:   # 하락 추세 필요
        return None

    c1, c2, c3 = candles[-3], candles[-2], candles[-1]

    # 조건 1: 3개 모두 양봉
    if not (c1.is_bull and c2.is_bull and c3.is_bull):
        return None

    # 조건 2: 각 캔들이 이전 몸통 내부에서 시가 시작
    if not (c1.open < c2.open < max(c1.open, c1.close)):
        return None
    if not (c2.open < c3.open < max(c2.open, c2.close)):
        return None

    # 조건 3: 종가 신고가 경신
    if not (c1.close < c2.close < c3.close):
        return None

    # 조건 4: 몸통 비율 >= 0.6
    body_ratios = [c1.body_ratio, c2.body_ratio, c3.body_ratio]
    if min(body_ratios) < 0.60:
        return None

    confidence = (
        min(body_ratios) / 0.60 * 0.5 +
        (c3.close - c1.open) / _avg_body([c1, c2, c3]) / 3 * 0.5
    )
    confidence = min(1.0, confidence)

    exp5, exp10, exp25 = PAPER_RETURNS["three_white_soldiers"]
    return PatternResult(
        pattern_name="three_white_soldiers",
        pattern_type="bullish",
        detected_at=c3.ts,
        confidence=confidence,
        candles_used=[vars(c) for c in [c1, c2, c3]],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )


def detect_three_black_crows(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    trend_candles = candles[-(lookback + 3) : -3]
    if _trend_slope(trend_candles) <= 0:   # 상승 추세 필요
        return None

    c1, c2, c3 = candles[-3], candles[-2], candles[-1]

    if not (c1.is_bear and c2.is_bear and c3.is_bear):
        return None
    if not (c1.close > c2.close > c3.close):
        return None
    if not (c2.open < c1.open and c3.open < c2.open):
        return None
    body_ratios = [c1.body_ratio, c2.body_ratio, c3.body_ratio]
    if min(body_ratios) < 0.60:
        return None

    confidence = min(1.0, min(body_ratios) / 0.60 * 0.6 + 0.4)
    exp5, exp10, exp25 = PAPER_RETURNS["three_black_crows"]
    return PatternResult(
        pattern_name="three_black_crows",
        pattern_type="bearish",
        detected_at=c3.ts,
        confidence=confidence,
        candles_used=[vars(c) for c in [c1, c2, c3]],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )
```

#### Morning Star / Evening Star

```python
def detect_morning_star(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    if _trend_slope(candles[-(lookback + 3) : -3]) >= 0:
        return None

    c1, c2, c3 = candles[-3], candles[-2], candles[-1]

    # c1: 긴 음봉
    if not c1.is_bear or c1.body_ratio < 0.70:
        return None
    # c2: 갭 다운 + 작은 몸통 (도지 허용)
    if max(c2.open, c2.close) >= min(c1.open, c1.close):
        return None
    # c3: 양봉이 c1 몸통의 50% 이상 되돌림
    if not c3.is_bull:
        return None
    recovery = (c3.close - c2.close) / c1.body
    if recovery < 0.50:
        return None

    confidence = min(1.0, recovery * 0.6 + c1.body_ratio * 0.4)
    exp5, exp10, exp25 = PAPER_RETURNS["morning_star"]
    return PatternResult(
        pattern_name="morning_star", pattern_type="bullish",
        detected_at=c3.ts, confidence=confidence,
        candles_used=[vars(c) for c in [c1, c2, c3]],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )


def detect_evening_star(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 3:
        return None
    if _trend_slope(candles[-(lookback + 3) : -3]) <= 0:
        return None

    c1, c2, c3 = candles[-3], candles[-2], candles[-1]

    if not c1.is_bull or c1.body_ratio < 0.70:
        return None
    if min(c2.open, c2.close) <= max(c1.open, c1.close):
        return None
    if not c3.is_bear:
        return None
    decline = (c2.open - c3.close) / c1.body
    if decline < 0.50:
        return None

    confidence = min(1.0, decline * 0.6 + c1.body_ratio * 0.4)
    exp5, exp10, exp25 = PAPER_RETURNS["evening_star"]
    return PatternResult(
        pattern_name="evening_star", pattern_type="bearish",
        detected_at=c3.ts, confidence=confidence,
        candles_used=[vars(c) for c in [c1, c2, c3]],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )
```

#### Hammer / Hanging Man

```python
def _is_hammer_shape(c: Candle) -> tuple[bool, float]:
    """망치형 형태 검사. (해당여부, confidence) 반환."""
    if c.range < 1e-6:
        return False, 0.0
    if c.lower_shadow < c.body * 2.0:        # 아래꼬리 >= 몸통의 2배
        return False, 0.0
    if c.upper_shadow > c.body * 0.3:         # 위꼬리 거의 없음
        return False, 0.0
    body_pos = (min(c.open, c.close) - c.low) / c.range
    if body_pos < 0.60:                        # 몸통이 상단 40% 이내
        return False, 0.0
    conf = min(1.0, (c.lower_shadow / (c.body * 2.0)) * 0.7 + body_pos * 0.3)
    return True, conf


def detect_hammer(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 1:
        return None
    if _trend_slope(candles[-(lookback + 1) : -1]) >= 0:  # 하락 추세 필요
        return None
    c = candles[-1]
    valid, conf = _is_hammer_shape(c)
    if not valid:
        return None
    exp5, exp10, exp25 = PAPER_RETURNS["hammer"]
    return PatternResult(
        pattern_name="hammer", pattern_type="bullish",
        detected_at=c.ts, confidence=conf,
        candles_used=[vars(c)],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )


def detect_hanging_man(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 1:
        return None
    if _trend_slope(candles[-(lookback + 1) : -1]) <= 0:  # 상승 추세 필요
        return None
    c = candles[-1]
    valid, conf = _is_hammer_shape(c)
    if not valid:
        return None
    exp5, exp10, exp25 = PAPER_RETURNS["hanging_man"]
    return PatternResult(
        pattern_name="hanging_man", pattern_type="bearish",
        detected_at=c.ts, confidence=conf,
        candles_used=[vars(c)],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )
```

#### Bullish / Bearish Engulfing

```python
def detect_bullish_engulfing(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 2:
        return None
    if _trend_slope(candles[-(lookback + 2) : -2]) >= 0:
        return None
    c1, c2 = candles[-2], candles[-1]
    if not c1.is_bear or not c2.is_bull:
        return None
    # c2 몸통이 c1 몸통을 완전히 감쌈
    if not (c2.open < c1.close and c2.close > c1.open):
        return None
    engulf_ratio = c2.body / (c1.body or 1e-9)
    confidence = min(1.0, (engulf_ratio - 1.0) * 0.5 + 0.5)
    exp5, exp10, exp25 = PAPER_RETURNS["bullish_engulfing"]
    return PatternResult(
        pattern_name="bullish_engulfing", pattern_type="bullish",
        detected_at=c2.ts, confidence=confidence,
        candles_used=[vars(c) for c in [c1, c2]],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )


def detect_bearish_engulfing(candles: list[Candle], lookback: int = 10) -> PatternResult | None:
    if len(candles) < lookback + 2:
        return None
    if _trend_slope(candles[-(lookback + 2) : -2]) <= 0:
        return None
    c1, c2 = candles[-2], candles[-1]
    if not c1.is_bull or not c2.is_bear:
        return None
    if not (c2.open > c1.close and c2.close < c1.open):
        return None
    engulf_ratio = c2.body / (c1.body or 1e-9)
    confidence = min(1.0, (engulf_ratio - 1.0) * 0.5 + 0.5)
    exp5, exp10, exp25 = PAPER_RETURNS["bearish_engulfing"]
    return PatternResult(
        pattern_name="bearish_engulfing", pattern_type="bearish",
        detected_at=c2.ts, confidence=confidence,
        candles_used=[vars(c) for c in [c1, c2]],
        expected_5=exp5, expected_10=exp10, expected_25=exp25,
    )
```

### 4-3. 통합 패턴 스캐너

```python
DETECTORS = [
    detect_three_white_soldiers,
    detect_morning_star,
    detect_hammer,
    detect_bullish_engulfing,
    detect_three_black_crows,
    detect_evening_star,
    detect_hanging_man,
    detect_bearish_engulfing,
]

class PatternDetector:
    def scan(
        self,
        raw_candles: list[dict],
        tf: Timeframe,
        min_confidence: float = 0.0,
        use_modern: bool = True,       # 최신 연구 방법론 적용 여부
        ml_model=None,                 # 선택적 ML 모델 (섹션 9 참조)
        strategy: "StrategyConfig | None" = None,   # 전략 혼합/선택 (섹션 10-1)
    ) -> list[PatternResult]:
        cfg = TIMEFRAME_CONFIGS[tf]
        candles = [Candle(**{k: v for k, v in r.items() if k in Candle.__dataclass_fields__})
                   for r in raw_candles]

        # ATR 계산 (최신 연구: 동적 임계값용)
        atr = _compute_atr(candles, period=14) if use_modern else None
        # 전략 미지정 시 균형 프리셋 사용 (고정 가중치 하드코딩 제거)
        strategy = strategy or STRATEGY_PRESETS["balanced"]

        results = []
        for detector in DETECTORS:
            result = detector(candles, lookback=cfg.trend_lookback)
            if not result:
                continue

            if use_modern:
                result.atr_normalized = _check_atr_threshold(candles, atr)
                result.volume_confirmed = _check_volume_spike(candles, period=20)
                if ml_model is not None:
                    feats = _extract_ml_features(candles, cfg.trend_lookback)
                    result.ml_score = float(ml_model.predict_proba([feats])[0][1])
                # composite_score는 StrategyConfig가 결정 — 혼합/선택 가능
                _apply_strategy(result, strategy)

            if result.confidence >= min_confidence:
                results.append(result)

        return sorted(results, key=lambda r: r.composite_score, reverse=True)
```

> **참고:** `mtf_score`는 비동기 조회가 필요하므로 `scan()` 이후 `MTFEngine.score()`로 채우고
> `_apply_strategy()`를 재호출하거나, 라우터에서 일괄 갱신한다 (섹션 9-3 참조).

---

## 5. 프론트엔드 구조 (SvelteKit)

### 5-1. 디렉토리 구조

```
frontend/
├── src/
│   ├── routes/
│   │   ├── +layout.svelte           # 네비게이션 + 타임프레임 글로벌 상태
│   │   ├── +page.svelte             # 대시보드 (시그널 목록)
│   │   ├── chart/[code]/
│   │   │   └── +page.svelte         # 캔들차트 + 패턴 마커
│   │   └── backtest/
│   │       └── +page.svelte         # 백테스트 설정 / 결과
│   ├── lib/
│   │   ├── components/
│   │   │   ├── CandleChart.svelte   # lightweight-charts 래퍼
│   │   │   ├── TimeframeSelector.svelte  # 타임프레임 선택 UI
│   │   │   ├── PatternBadge.svelte
│   │   │   ├── SignalTable.svelte
│   │   │   └── BacktestResult.svelte
│   │   ├── stores/
│   │   │   └── timeframe.ts         # 전역 타임프레임 상태 (writable store)
│   │   └── api.ts                   # fetch 래퍼
│   └── app.html
├── package.json
└── svelte.config.js
```

### 5-2. 타임프레임 전역 상태 (stores/timeframe.ts)

```typescript
import { writable } from 'svelte/store';

export type Timeframe = '1m' | '3m' | '5m' | '10m' | '15m' | '30m' | '60m' | '1d';

export const TIMEFRAME_LABELS: Record<Timeframe, string> = {
  '1m':  '1분',
  '3m':  '3분',
  '5m':  '5분',
  '10m': '10분',
  '15m': '15분',
  '30m': '30분',
  '60m': '60분',
  '1d':  '일봉',
};

export const selectedTimeframe = writable<Timeframe>('5m');
```

### 5-3. 타임프레임 선택 컴포넌트 (TimeframeSelector.svelte)

```svelte
<script lang="ts">
  import { selectedTimeframe, TIMEFRAME_LABELS, type Timeframe } from '$lib/stores/timeframe';

  const timeframes = Object.keys(TIMEFRAME_LABELS) as Timeframe[];
</script>

<div class="tf-selector">
  {#each timeframes as tf}
    <button
      class:active={$selectedTimeframe === tf}
      on:click={() => selectedTimeframe.set(tf)}
    >
      {TIMEFRAME_LABELS[tf]}
    </button>
  {/each}
</div>

<style>
  .tf-selector { display: flex; gap: 4px; }
  button {
    padding: 4px 10px;
    border: 1px solid #444;
    background: #1e1e2e;
    color: #cdd6f4;
    border-radius: 4px;
    cursor: pointer;
  }
  button.active {
    background: #89b4fa;
    color: #1e1e2e;
    border-color: #89b4fa;
  }
</style>
```

### 5-4. 캔들 차트 컴포넌트 (CandleChart.svelte)

```svelte
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { createChart, type IChartApi, type ISeriesApi } from 'lightweight-charts';
  import type { PatternResult } from '$lib/api';

  export let candles: { ts: string; open: number; high: number; low: number; close: number }[] = [];
  export let patterns: PatternResult[] = [];

  let container: HTMLDivElement;
  let chart: IChartApi;
  let series: ISeriesApi<'Candlestick'>;

  onMount(() => {
    chart = createChart(container, {
      layout: { background: { color: '#1e1e2e' }, textColor: '#cdd6f4' },
      grid: { vertLines: { color: '#313244' }, horzLines: { color: '#313244' } },
      width: container.clientWidth,
      height: 480,
    });

    series = chart.addCandlestickSeries({
      upColor: '#a6e3a1', downColor: '#f38ba8',
      borderUpColor: '#a6e3a1', borderDownColor: '#f38ba8',
      wickUpColor: '#a6e3a1', wickDownColor: '#f38ba8',
    });

    // 캔들 데이터 설정
    series.setData(candles.map(c => ({
      time: c.ts,
      open: c.open, high: c.high, low: c.low, close: c.close,
    })));

    // 패턴 마커 오버레이
    const markers = patterns.map(p => ({
      time: p.detected_at,
      position: p.pattern_type === 'bullish' ? 'belowBar' : 'aboveBar',
      color: p.pattern_type === 'bullish' ? '#a6e3a1' : '#f38ba8',
      shape: p.pattern_type === 'bullish' ? 'arrowUp' : 'arrowDown',
      text: `${p.pattern_name} (${(p.confidence * 100).toFixed(0)}%)`,
    }));
    series.setMarkers(markers);
  });

  onDestroy(() => chart?.remove());
</script>

<div bind:this={container} style="width:100%" />
```

### 5-5. 대시보드 페이지 (+page.svelte)

```svelte
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { selectedTimeframe } from '$lib/stores/timeframe';
  import TimeframeSelector from '$lib/components/TimeframeSelector.svelte';
  import SignalTable from '$lib/components/SignalTable.svelte';
  import { fetchSignals, type SignalItem } from '$lib/api';

  let signals: SignalItem[] = [];
  let loading = false;
  let intervalId: ReturnType<typeof setInterval>;

  async function refresh() {
    loading = true;
    signals = await fetchSignals({ tf: $selectedTimeframe, min_conf: 0.7 });
    loading = false;
  }

  $: $selectedTimeframe, refresh();   // 타임프레임 변경 시 자동 갱신

  onMount(() => {
    refresh();
    intervalId = setInterval(refresh, 30_000);  // 30초 폴링
  });

  onDestroy(() => clearInterval(intervalId));
</script>

<main>
  <header>
    <h1>Caginalp Pattern Signals</h1>
    <TimeframeSelector />
  </header>
  {#if loading}<p>스캔 중...</p>{/if}
  <SignalTable {signals} />
</main>
```

---

## 6. 하네스 엔지니어링

### 6-1. 개발 환경 설정

#### pyproject.toml (백엔드)

```toml
[project]
name = "candlestick-trader-backend"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = [
  "fastapi>=0.115",
  "uvicorn[standard]>=0.30",
  "aiohttp>=3.10",
  "numpy>=2.0",
  "pydantic-settings>=2.0",
  "apscheduler>=3.10",     # 장 마감 후 배치 스캔 스케줄러 (섹션 11-5)
]

[project.optional-dependencies]
ml = [                     # Phase 4에서만 설치 — MVP에는 불필요
  "scikit-learn>=1.5",
  "joblib>=1.4",
  # "torch>=2.4",          # GAF-CNN 사용 시에만
]
test = [
  "pytest>=8.0",
  "pytest-asyncio>=0.24",
  "httpx>=0.27",           # FastAPI TestClient 비동기 지원
  "respx>=0.21",           # aiohttp mock
]
lint = [
  "ruff>=0.6",
  "mypy>=1.11",
]

[tool.pytest.ini_options]
asyncio_mode = "auto"

[tool.ruff]
line-length = 100
select = ["E", "F", "I", "UP"]
```

#### package.json (프론트엔드)

```json
{
  "name": "candlestick-trader-frontend",
  "private": true,
  "scripts": {
    "dev":   "vite dev",
    "build": "vite build",
    "check": "svelte-kit sync && svelte-check --tsconfig ./tsconfig.json",
    "lint":  "eslint src"
  },
  "devDependencies": {
    "@sveltejs/adapter-node": "^5.2",
    "@sveltejs/kit": "^2.0",
    "svelte": "^5.0",
    "vite": "^5.0",
    "typescript": "^5.0",
    "svelte-check": "^3.8"
  },
  "dependencies": {
    "lightweight-charts": "^4.2"
  }
}
```

### 6-2. 테스트 하네스

#### 픽스처 설계 (tests/fixtures/)

```python
# tests/fixtures/candles.py
"""타임프레임별 테스트용 픽스처 캔들 데이터."""

import json
from pathlib import Path

FIXTURES_DIR = Path(__file__).parent

def load_fixture(name: str) -> list[dict]:
    return json.loads((FIXTURES_DIR / f"{name}.json").read_text())


# 각 픽스처 파일: {pattern_name}_{timeframe}.json
# 예: three_white_soldiers_5m.json, morning_star_1d.json
# 구조: 패턴이 마지막 N봉에서 발생하도록 구성된 OHLCV 배열
```

#### 패턴 감지 단위 테스트

```python
# tests/test_pattern_detector.py

import pytest
from app.services.pattern_detector import PatternDetector, Timeframe
from tests.fixtures.candles import load_fixture

@pytest.fixture
def detector():
    return PatternDetector()

@pytest.mark.parametrize("pattern,tf", [
    ("three_white_soldiers", "5m"),
    ("three_white_soldiers", "1d"),
    ("morning_star",         "5m"),
    ("hammer",               "15m"),
    ("bullish_engulfing",    "60m"),
    ("three_black_crows",    "5m"),
    ("evening_star",         "1d"),
    ("hanging_man",          "30m"),
    ("bearish_engulfing",    "5m"),
])
def test_detects_pattern(detector, pattern, tf):
    candles = load_fixture(f"{pattern}_{tf}")
    results = detector.scan(candles, Timeframe(tf))
    names = [r.pattern_name for r in results]
    assert pattern in names, f"{pattern} not detected in {tf} fixture"

@pytest.mark.parametrize("pattern,tf", [
    ("hammer",       "5m"),   # 추세 조건 불충족 (상승 추세에 hammer)
    ("hanging_man",  "1d"),
])
def test_rejects_wrong_trend(detector, pattern, tf):
    candles = load_fixture(f"{pattern}_{tf}_wrong_trend")
    results = detector.scan(candles, Timeframe(tf))
    names = [r.pattern_name for r in results]
    assert pattern not in names
```

#### API 통합 테스트

```python
# tests/test_api_patterns.py

import pytest
from httpx import AsyncClient, ASGITransport
from unittest.mock import AsyncMock, patch
from app.main import app

@pytest.fixture
async def client():
    async with AsyncClient(transport=ASGITransport(app), base_url="http://test") as c:
        yield c

async def test_patterns_endpoint_returns_results(client):
    mock_candles = [...]  # 패턴 포함 픽스처

    with patch("app.services.candle_fetcher.CandleFetcher.fetch", new_callable=AsyncMock) as mock:
        mock.return_value = mock_candles
        resp = await client.get("/api/patterns/005930?tf=5m")

    assert resp.status_code == 200
    body = resp.json()
    assert "patterns" in body
    assert isinstance(body["patterns"], list)
```

### 6-3. 런타임 하네스

#### Rate Limit 준수 (분봉 다중 스캔 시)

```python
# routers/signals.py — 전종목 스캔 시 세마포어로 동시성 제어

import asyncio
from fastapi import APIRouter, Depends, Query
from app.dependencies import get_ebest, get_fetcher

router = APIRouter()
_SCAN_SEM = asyncio.Semaphore(5)   # eBest rate limit 1.1s/TR 기준 최대 5 동시

@router.get("")
async def list_signals(
    market: str = Query("ALL"),
    tf: str  = Query("5m"),
    min_conf: float = Query(0.7),
    limit: int = Query(30),
    ebest=Depends(get_ebest),
    fetcher=Depends(get_fetcher),
):
    token = await ebest.auth_token()
    stocks = await ebest.get_all_stocks(token, gubun="0")
    if market != "ALL":
        stocks = [s for s in stocks if s["market"] == market]

    detector = PatternDetector()
    all_signals = []

    async def scan_one(stock):
        async with _SCAN_SEM:
            candles = await fetcher.fetch(token, stock["code"], Timeframe(tf))
            patterns = detector.scan(candles, Timeframe(tf), min_confidence=min_conf)
            return stock, patterns

    results = await asyncio.gather(*[scan_one(s) for s in stocks], return_exceptions=True)

    for item in results:
        if isinstance(item, Exception):
            continue
        stock, patterns = item
        for p in patterns:
            all_signals.append({"shcode": stock["code"], "name": stock["name"], **vars(p)})

    all_signals.sort(key=lambda x: x["confidence"], reverse=True)
    return all_signals[:limit]
```

#### 백테스트 — 거래비용 반영 + Look-ahead Bias 방지 (현실화 버전)

> ⚠️ 거래비용을 빼지 않은 백테스트는 **실전에서 반드시 손실**로 이어진다.  
> 한국 주식 왕복 비용 가정: 매도세 0.15%(2025 인하) + 수수료 왕복 ~0.03% + 슬리피지 ~0.17% ≈ **0.35%**.

```python
# services/backtest_engine.py
from dataclasses import dataclass

@dataclass
class CostModel:
    """한국 주식 거래비용 모델 (왕복 기준)."""
    fee_rate:      float = 0.00015   # 위탁수수료 (편도, 증권사별 상이)
    tax_rate:      float = 0.0015    # 증권거래세 (매도 시, 2025년 0.15%)
    slippage_rate: float = 0.0008    # 진입/청산 각각 적용되는 슬리피지

    def round_trip_cost(self) -> float:
        """진입+청산 총 비용 비율. 진입가 대비 % 차감용."""
        return (self.fee_rate * 2) + self.tax_rate + (self.slippage_rate * 2)


def run_backtest(
    candles: list[dict],
    pattern_name: str,
    tf: Timeframe,
    hold_bars: int,
    cost: CostModel = CostModel(),
    side: str = "long",            # "long"=상승패턴 매수, "short"=하락패턴 (공매도/회피)
    detector: "PatternDetector | None" = None,
) -> dict:
    """
    규칙:
    - 패턴 감지 봉의 '다음 봉 시가'로 진입 (현실적: 종가 확정 후에야 신호 확정)
    - hold_bars 봉 후 종가로 청산
    - 모든 수익률에서 round_trip_cost 차감
    - i+1 이후 데이터는 감지 시점에서 접근 불가 (슬라이스로 강제)
    """
    detector = detector or PatternDetector()
    rt_cost = cost.round_trip_cost() * 100   # %
    returns: list[float] = []

    # 성능: 매 캔들 전체 재스캔(O(n²)) 대신 마지막 N봉만 슬라이스 (섹션 11-2 참조)
    for i in range(len(candles) - hold_bars - 1):
        window = candles[max(0, i - 40) : i + 1]   # 패턴 판정에 충분한 최근 봉만
        found = detector.scan(window, tf, use_modern=False)
        if pattern_name not in {p.pattern_name for p in found}:
            continue
        entry = candles[i + 1]["open"]             # 다음 봉 시가 진입
        exit_ = candles[i + 1 + hold_bars]["close"]
        gross = (exit_ - entry) / entry * 100
        if side == "short":
            gross = -gross
        returns.append(gross - rt_cost)            # 비용 차감 후 순수익

    if not returns:
        return {"signals": 0, "win_rate": 0.0, "avg_return": 0.0,
                "max_drawdown": 0.0, "sharpe_ratio": 0.0, "profit_factor": 0.0}

    arr = np.array(returns)
    wins = arr[arr > 0]
    losses = arr[arr <= 0]
    equity = np.cumsum(arr)
    peak = np.maximum.accumulate(equity)
    mdd = float(np.min(equity - peak)) if len(equity) else 0.0
    pf = float(wins.sum() / abs(losses.sum())) if losses.sum() != 0 else float("inf")

    return {
        "signals":      len(returns),
        "win_rate":     float(len(wins) / len(arr)),
        "avg_return":   float(arr.mean()),          # 거래비용 차감 후 평균 %
        "max_drawdown": mdd,
        "sharpe_ratio": float(arr.mean() / (arr.std() + 1e-9)),
        "profit_factor": pf,                        # 총이익/총손실, >1.3 이면 실전 후보
    }
```

**현실화 핵심 변경점:**
- ✅ **다음 봉 시가 진입**: 종가 확정 시점에 신호가 나오므로 그 봉 종가로는 진입 불가 (look-ahead)
- ✅ **거래비용 차감**: `CostModel`로 수수료·세금·슬리피지 반영
- ✅ **profit_factor 추가**: 승률보다 실전 생존성을 잘 보여주는 지표 (>1.3 권장)
- ✅ **윈도우 슬라이스**: O(n²)→O(n×40)으로 백테스트 속도 개선

### 6-4. 환경 변수 & 설정

#### .env.example

```dotenv
EBEST_APP_KEY=your_app_key_here
EBEST_APP_SECRET=your_app_secret_here
# 실전: https://openapi.ebestsec.co.kr  /  모의투자: https://openapivts.ebestsec.co.kr
EBEST_URL=https://openapi.ebestsec.co.kr
EBEST_VERIFY_SSL=true

# 서버
BACKEND_HOST=0.0.0.0
BACKEND_PORT=8000

# 자동매매 (섹션 10) — 안전 기본값
TRADING_MODE=paper            # paper | live  (실전 전환은 신중히!)
TRADING_DEFAULT_STRATEGY=balanced
TRADING_MAX_POSITIONS=5
TRADING_RISK_PER_TRADE=0.01   # 1R = 자본의 1%
TRADING_DAILY_LOSS_LIMIT=0.03 # 일일 -3% 도달 시 정지
TRADING_PAPER_SEED=10000000   # 페이퍼 트레이딩 가상 시드(원)

# 프론트엔드
PUBLIC_API_BASE=http://localhost:8000
```

> 🔐 **실전(`live`) 전환 전 체크:** 섹션 11-2 실전 투입 체크리스트를 모두 통과했는가?
> `EBEST_URL`을 모의투자 도메인으로 두고 `TRADING_MODE=paper`로 충분히 검증한 뒤 전환하라.

#### config.py

```python
from functools import lru_cache
from pydantic_settings import BaseSettings

class Settings(BaseSettings):
    EBEST_APP_KEY:    str
    EBEST_APP_SECRET: str
    EBEST_URL:        str = "https://openapi.ebestsec.co.kr"
    EBEST_VERIFY_SSL: bool = True

    # 자동매매 (섹션 10)
    TRADING_MODE:             str   = "paper"   # paper | live
    TRADING_DEFAULT_STRATEGY: str   = "balanced"
    TRADING_MAX_POSITIONS:    int   = 5
    TRADING_RISK_PER_TRADE:   float = 0.01
    TRADING_DAILY_LOSS_LIMIT: float = 0.03
    TRADING_PAPER_SEED:       float = 10_000_000

    class Config:
        env_file = ".env"

@lru_cache
def get_settings() -> Settings:
    return Settings()
```

---

## 7. 프롬프트 엔지니어링 가이드

### 7-1. 구현 작업별 프롬프트 템플릿

아래는 각 모듈을 AI 코딩 어시스턴트에게 구현 요청 시 사용할 정밀 프롬프트다.

---

#### [프롬프트 A] 패턴 감지 서비스 구현

```
다음 조건으로 `app/services/pattern_detector.py`를 구현하라.

## 전제
- `Candle` dataclass: ts, open, high, low, close, volume 필드. 
  body, range, body_ratio, upper_shadow, lower_shadow, is_bull, is_bear 프로퍼티 포함.
- `PatternResult` dataclass: pattern_name, pattern_type("bullish"|"bearish"),
  detected_at, confidence(0~1), candles_used, expected_5, expected_10, expected_25.
- `PAPER_RETURNS`: 논문(Caginalp & Laurent 1998) 수치 하드코딩 딕셔너리.

## 구현할 8개 함수 시그니처
각 함수는 `(candles: list[Candle], lookback: int = 10) -> PatternResult | None` 형태.
- detect_three_white_soldiers / detect_three_black_crows
- detect_morning_star / detect_evening_star
- detect_hammer / detect_hanging_man
- detect_bullish_engulfing / detect_bearish_engulfing

## 공통 규칙
1. 추세 판단: _trend_slope(candles[-(lookback+n):-n])로 선형회귀 기울기 계산.
   상승반전 패턴은 slope<0 필요, 하락반전은 slope>0 필요.
2. confidence는 0.0~1.0으로 min() 클램핑.
3. 0 나눔 방지: range 계산 시 `or 1e-9` 추가.
4. numpy 사용 허용 (polyfit).
5. 주석 없음. 타입 힌트 필수.
```

---

#### [프롬프트 B] 타임프레임 추상화 레이어 구현

```
`app/services/candle_fetcher.py`를 구현하라.

## 요구사항
- `Timeframe` Enum: "1m","3m","5m","10m","15m","30m","60m","1d"
- `TimeframeConfig` dataclass: timeframe, tr_code, ncnt, qrycnt,
  trend_lookback, hold_bars(list[int]), label
- `TIMEFRAME_CONFIGS` dict: 위 표 참조
- `CandleFetcher` 클래스:
  - __init__(self, ebest: EBestService)
  - async fetch(token, shcode, tf, sdate="", stime="", edate="", etime="") -> list[dict]
    * tf == Timeframe.D1 → ebest.fetch_daily_candles() 호출
    * 그 외 → ebest.fetch_candle_data_tr(ncnt=cfg.ncnt, ...) 호출 후 _normalize_t8412()
  - @staticmethod _normalize_t8412(raw, ncnt) -> list[dict]
    * t8412OutBlock1 파싱, ts 필드(date+time 결합), 날짜 오름차순 정렬
    * 필드: date, time, ts, open, high, low, close, volume

## ebest_service.py 참조
- fetch_daily_candles(token, shcode, qrycnt) → list[dict] (이미 오름차순)
- fetch_candle_data_tr(token, shcode, ncnt, qrycnt, nday, sdate, stime, edate, etime) → dict
  응답 키: t8412OutBlock1 (list), 각 항목: date, time, open, high, low, close, jdiff_vol
```

---

#### [프롬프트 C] SvelteKit 차트 페이지 구현

```
`src/routes/chart/[code]/+page.svelte`를 구현하라.

## 레이아웃
- 상단: 종목명(code) + TimeframeSelector 컴포넌트
- 중앙: CandleChart 컴포넌트 (lightweight-charts, 너비 100%, 높이 480px)
  - 패턴 감지 봉에 마커 표시:
    bullish → belowBar, color="#a6e3a1", shape="arrowUp", text="{pattern_name}({confidence%})"
    bearish → aboveBar, color="#f38ba8", shape="arrowDown"
- 우측 사이드바(280px): 감지된 패턴 목록 카드
  - 각 카드: 패턴명, 타입 뱃지, 신뢰도 바, 기대수익(5봉/10봉/25봉)

## 데이터 흐름
- page load: `selectedTimeframe` store 구독
- 타임프레임 변경 시 GET /api/candles/{code}?tf={tf} 재호출
- 동시에 GET /api/patterns/{code}?tf={tf} 호출
- lightweight-charts의 setData / setMarkers로 렌더링 업데이트

## 스타일 기준
- 배경: #1e1e2e (Catppuccin Mocha)
- 텍스트: #cdd6f4
- 카드 구분선: #313244
- Tailwind 사용 금지, scoped CSS 작성
```

---

#### [프롬프트 D] 백테스트 엔진 구현

```
`app/services/backtest_engine.py`를 구현하라.

## 함수 시그니처
def run_backtest(
    candles: list[dict],
    pattern_name: str,
    tf: Timeframe,
    hold_bars: int,           # 5, 10, 25
) -> dict

## 알고리즘 (Look-ahead bias 방지 필수)
for i in range(len(candles) - hold_bars):
    visible = candles[:i + 1]         # i 시점까지만 노출
    detected = PatternDetector().scan(visible, tf)
    if pattern_name not in [p.pattern_name for p in detected]:
        continue
    entry_price = candles[i]["close"]
    exit_price  = candles[i + hold_bars]["close"]
    return_pct  = (exit_price - entry_price) / entry_price * 100
    → 결과 누적

## 반환 형식
{
  "signals":     int,
  "win_rate":    float,   # 수익 거래 비율
  "avg_return":  float,   # 평균 수익률(%)
  "max_drawdown": float,  # 최대 낙폭(%)
  "sharpe_ratio": float,  # 샤프 비율 (무위험 수익률 0 가정)
  "by_pattern":  []       # pattern_name별 소계
}
```

---

### 7-2. 코드 리뷰 체크리스트 (AI 어시스턴트용)

```
패턴 감지 코드 리뷰 시 다음 항목을 확인하라:

── Caginalp 기본 규칙 ──────────────────────────────────────────
□ 모든 나눔 연산에 0 가드 (or 1e-9, or 빈 리스트 조기 반환)
□ 추세 판단 슬라이스가 패턴 캔들을 포함하지 않음 (candles[:-n] 형태)
□ t8412 분봉은 "ts" 필드로 정렬 (date+time 결합), t8410 일봉은 "date"로 정렬
□ asyncio.Semaphore(5)가 전종목 스캔 경로에 적용됨
□ look-ahead bias 방지: 백테스트 루프에서 visible = candles[:i+1]
□ confidence 반환값이 min(1.0, ...) 로 클램핑됨
□ PatternResult.candles_used가 원본 dict 복사본임 (Candle 객체 직접 저장 금지)
□ 타임프레임 D1은 t8410, 그 외는 t8412 경로를 탐

── 최신 연구 확장 (섹션 9) ─────────────────────────────────────
□ _compute_atr(): period+1 이상의 캔들 필요, 부족 시 0.0 반환
□ _check_volume_spike(): period=10 (분봉) / 20 (일봉) 구분 적용
□ MTFEngine.score(): 상위 타임프레임 조회도 Semaphore 내에서 실행
□ _extract_ml_features(): 반환 리스트 길이 항상 15개 (부족 시 [0.0]*15)
□ permutation_test(): n_signals < 5 이면 조기 반환
□ fdr_correction(): p_value=None인 패턴 제외 후 교정
□ GAF 인코딩: s_norm np.clip(-1, 1) 후 arccos 적용 (수치 안정성)
□ composite_score = conf*0.3 + mtf*0.3 + ml*0.4 (합산이 1.0 초과 불가)
```

---

## 8. eBest API 연동 사양

### 8-1. TR별 분봉 데이터 구조

#### t8412 요청 (fetch_candle_data_tr)

```json
{
  "t8412InBlock": {
    "shcode": "005930",
    "ncnt":   5,
    "qrycnt": 300,
    "nday":   "1",
    "sdate":  "",
    "stime":  "",
    "edate":  "",
    "etime":  "",
    "comp_yn": "N"
  }
}
```

#### t8412 응답 주요 필드

```
t8412OutBlock1[]:
  date      : "20260617"    ← YYYYMMDD
  time      : "093000"      ← HHMMSS
  open      : 73400
  high      : 73800
  low       : 73200
  close     : 73600
  jdiff_vol : 12345          ← 거래량 (volume으로 매핑)
```

#### t8410 일봉 응답 주요 필드

```
t8410OutBlock1[]:
  date      : "20260617"
  open      : 73400
  high      : 73800
  low       : 73200
  close     : 73600
  jdiff_vol : 98765
```

### 8-2. Rate Limit 설계

```
TR 코드        호출 간격    최대 동시 스캔 종목
──────────────────────────────────────────────
t8412 (분봉)   1.1초/콜    Semaphore(5) ≈ 4.5콜/초
t8410 (일봉)   1.1초/콜    Semaphore(5)
t8436 (종목)   1.1초/콜    1회 호출 (전종목)
t0424 (잔고)   1.1초/콜    1회 호출
CSPAT00601     1.1초/콜    주문 시 단독
```

> 전종목(약 2,500개) 스캔 시 예상 소요 시간:  
> 분봉: 2500 ÷ (5콜/1.1초) ≈ **550초 (약 9분)**  
> → `/api/signals` 엔드포인트는 캐시(TTL 60초) 또는 백그라운드 태스크로 운영 권장

### 8-3. 권장 운영 방식

```
분봉 실시간 스캔:  관심 종목 리스트(최대 50종목) 대상으로만 스캔
일봉 전종목 스캔:  장 마감 후(16:00 KST) 배치 실행
백테스트:         일봉 데이터 우선 사용 (논문 원본 타임프레임)
```

---

## 9. 최신 연구 방법론 통합 (2019–2024)

> 이 섹션은 기존 Caginalp 8패턴 위에 **레이어** 형태로 추가된다.  
> `PatternDetector.scan(use_modern=True)` 플래그로 활성화.

---

### 9-1. ATR 적응형 임계값 (Adaptive ATR Threshold)

**출처:** IJRPR 2024 Systematic Review; Marshall et al. (2006) 확장 적용

고정 비율(예: body_ratio ≥ 0.60)은 타임프레임·종목 변동성에 무관하게 동일 기준을 적용하여
저변동 구간에서 과검출, 고변동 구간에서 미검출이 발생한다.  
ATR 정규화는 현재 캔들의 몸통·꼬리를 최근 N봉 Average True Range로 나눠 비교한다.

```python
# services/pattern_detector.py 추가

def _compute_atr(candles: list[Candle], period: int = 14) -> float:
    """True Range의 단순 이동 평균 (ATR)."""
    if len(candles) < period + 1:
        return 0.0
    trs = []
    for i in range(1, period + 1):
        c = candles[-(i)]
        p = candles[-(i + 1)]
        tr = max(c.high - c.low, abs(c.high - p.close), abs(c.low - p.close))
        trs.append(tr)
    return float(np.mean(trs)) or 1e-9


def _check_atr_threshold(
    candles: list[Candle],
    atr: float,
    min_body_atr_ratio: float = 0.3,  # 몸통이 ATR의 30% 이상이어야 유효
) -> bool:
    """마지막 캔들의 몸통이 ATR 기준으로 충분한지 확인."""
    if not candles or atr <= 0:
        return False
    return candles[-1].body >= atr * min_body_atr_ratio
```

**PatternDetector 통합 지점:**  
각 `detect_*` 함수 내부의 `body_ratio >= 0.60` 조건을  
`body >= atr * 0.3` 으로 대체(또는 병렬 적용)하여  
`PatternResult.atr_normalized = True` 마킹.

---

### 9-2. 거래량 확인 필터 (Volume Confirmation)

**출처:** PLOS ONE 2020 — *Improving stock trading decisions based on pattern recognition using ML*

패턴 발생 시 거래량이 평균 대비 1.5배 이상이면 이탈·반전 신호의 신뢰도가 통계적으로 유의하게 상승.

```python
def _check_volume_spike(
    candles: list[Candle],
    period: int = 20,
    threshold: float = 1.5,
) -> bool:
    """
    마지막 캔들의 거래량이 직전 period봉 평균의 threshold배 이상인지 확인.
    분봉의 경우 장 초반 거래량 왜곡을 고려해 period=10 으로 줄여 사용 권장.
    """
    if len(candles) < period + 1:
        return False
    recent_vols = [c.volume for c in candles[-(period + 1) : -1]]
    avg_vol = np.mean(recent_vols) or 1e-9
    return candles[-1].volume >= avg_vol * threshold
```

**시그널 우선순위에서 활용:**  
`volume_confirmed=True` 인 패턴을 대시보드 상단에 노출.  
`composite_score` 계산 시 볼륨 확인 여부로 +0.1 가중.

---

### 9-3. 멀티타임프레임 컨플루언스 (MTF Confluence)

**출처:** IEEE Xplore 2022 — *Stock Trend Prediction Using Candlestick Charting and Ensemble ML*

동일 패턴이 여러 타임프레임에서 동시 발생하면 단일 타임프레임 대비  
평균 수익률이 통계적으로 유의하게 개선됨 (Sharpe ratio 0.81 → 1.34).

#### MTF 스코어 계산 로직

```python
# services/mtf_engine.py

MTF_GROUPS: dict[Timeframe, list[Timeframe]] = {
    Timeframe.M5:  [Timeframe.M15, Timeframe.H1],
    Timeframe.M15: [Timeframe.H1,  Timeframe.D1],
    Timeframe.H1:  [Timeframe.D1],
    Timeframe.D1:  [],   # 일봉은 상위 타임프레임 없음
}

class MTFEngine:
    def __init__(self, fetcher: CandleFetcher, detector: PatternDetector):
        self._fetcher  = fetcher
        self._detector = detector

    async def score(
        self,
        token: str,
        shcode: str,
        base_tf: Timeframe,
        pattern_name: str,
    ) -> float:
        """
        base_tf에서 패턴 발생 시, 상위 타임프레임에서도 동일 방향 패턴이
        발생했는지 확인. 일치하는 타임프레임 수 / 전체 체크 수 로 점수 반환.
        """
        upper_tfs = MTF_GROUPS.get(base_tf, [])
        if not upper_tfs:
            return 0.5   # 상위 없으면 중립

        hits = 0
        for tf in upper_tfs:
            candles = await self._fetcher.fetch(token, shcode, tf)
            results = self._detector.scan(candles, tf, min_confidence=0.5)
            matched_type = next(
                (r.pattern_type for r in results if r.pattern_name == pattern_name), None
            )
            # 기준 패턴과 동일 방향이면 점수 부여
            if matched_type is not None:
                hits += 1

        return hits / len(upper_tfs)
```

#### API 연동 — `/api/patterns/{shcode}` 확장

```python
# routers/patterns.py 수정

@router.get("/{shcode}")
async def get_patterns(
    shcode: str,
    tf: str = Query("5m"),
    mtf: bool = Query(False),      # 멀티타임프레임 컨플루언스 계산 여부
    ebest=Depends(get_ebest),
    fetcher=Depends(get_fetcher),
):
    token = await ebest.auth_token()
    candles = await fetcher.fetch(token, shcode, Timeframe(tf))
    results = PatternDetector().scan(candles, Timeframe(tf))

    if mtf:
        engine = MTFEngine(fetcher, PatternDetector())
        for r in results:
            r.mtf_score = await engine.score(token, shcode, Timeframe(tf), r.pattern_name)
            r.composite_score = r.confidence * 0.3 + r.mtf_score * 0.3 + r.ml_score * 0.4

    return {"shcode": shcode, "timeframe": tf, "patterns": [vars(r) for r in results]}
```

---

### 9-4. ML 앙상블 신뢰도 스코어 (Ensemble ML Scoring)

**출처:** IEEE 2022 — Random Forest + SVM 앙상블; 연 평균 수익률 36.73%, Sharpe 0.81

#### 피처 벡터 설계

```python
def _extract_ml_features(candles: list[Candle], lookback: int) -> list[float]:
    """
    Random Forest 입력 피처. 총 15개 수치 피처.
    논문(IEEE 2022) Table 2 기준 + ATR 정규화 확장.
    """
    if len(candles) < lookback + 3:
        return [0.0] * 15

    recent = candles[-(lookback + 3):]
    last   = candles[-1]
    atr    = _compute_atr(candles) or 1e-9

    return [
        # 캔들 형태 피처 (6개)
        last.body / atr,                              # ATR 대비 몸통 크기
        last.upper_shadow / atr,                      # ATR 대비 위꼬리
        last.lower_shadow / atr,                      # ATR 대비 아래꼬리
        last.body_ratio,                              # 몸통 비율
        float(last.is_bull),                          # 양봉 여부
        (last.close - last.open) / (last.open or 1), # 등락률

        # 추세 피처 (3개)
        _trend_slope(candles[-lookback:]) / atr,      # 기울기 (ATR 정규화)
        np.mean([c.close for c in recent[-5:]]) / last.close - 1,   # 5봉 평균 대비
        np.mean([c.close for c in recent[-10:]]) / last.close - 1,  # 10봉 평균 대비

        # 거래량 피처 (3개)
        last.volume / (np.mean([c.volume for c in recent[-20:]]) or 1),  # 거래량 비율
        np.std([c.volume for c in recent[-10:]]) / (last.volume or 1),   # 거래량 변동성
        float(last.volume > np.mean([c.volume for c in recent[-5:]])),   # 거래량 스파이크

        # 가격 위치 피처 (3개)
        (last.close - min(c.low for c in recent[-14:])) /
        ((max(c.high for c in recent[-14:]) - min(c.low for c in recent[-14:])) or 1),
        last.high / (max(c.high for c in recent[-5:]) or 1),
        last.low  / (min(c.low  for c in recent[-5:]) or 1),
    ]
```

#### 모델 학습 & 저장 파이프라인

```python
# scripts/train_ml_model.py
# 실행: python scripts/train_ml_model.py --shcodes 005930 035420 ...

import joblib
import asyncio
import numpy as np
from sklearn.ensemble import RandomForestClassifier
from sklearn.svm import SVC
from sklearn.pipeline import Pipeline
from sklearn.preprocessing import StandardScaler
from sklearn.calibration import CalibratedClassifierCV

async def build_dataset(codes: list[str], hold_bars: int = 5):
    """일봉 데이터로 지도 학습 데이터셋 생성."""
    X, y = [], []
    ebest = EBestService()
    await ebest.start()
    token = await ebest.auth_token()
    fetcher = CandleFetcher(ebest)
    detector = PatternDetector()

    for code in codes:
        candles_raw = await fetcher.fetch(token, code, Timeframe.D1)
        candles = [Candle(**{k: v for k, v in r.items()
                             if k in Candle.__dataclass_fields__}) for r in candles_raw]

        for i in range(10, len(candles) - hold_bars):
            visible = candles[:i + 1]
            results = detector.scan(candles_raw[:i + 1], Timeframe.D1)
            if not results:
                continue
            feats = _extract_ml_features(visible, lookback=10)
            future_return = (candles[i + hold_bars].close - candles[i].close) / candles[i].close
            label = 1 if future_return > 0 else 0
            X.append(feats)
            y.append(label)

    await ebest.aclose()
    return np.array(X), np.array(y)

def train(X, y):
    rf = RandomForestClassifier(n_estimators=200, max_depth=8, random_state=42)
    pipeline = Pipeline([
        ("scaler", StandardScaler()),
        ("clf", CalibratedClassifierCV(rf, cv=5)),   # 확률 캘리브레이션
    ])
    pipeline.fit(X, y)
    joblib.dump(pipeline, "models/candlestick_rf.joblib")
    print(f"학습 완료. 샘플 수: {len(y)}, 양성 비율: {y.mean():.2%}")
```

#### 모델 서빙 (FastAPI lifespan 통합)

```python
# main.py lifespan 수정
import joblib
from pathlib import Path

@asynccontextmanager
async def lifespan(app: FastAPI):
    ebest = EBestService()
    await ebest.start()
    app.state.ebest   = ebest
    app.state.fetcher = CandleFetcher(ebest)

    model_path = Path("models/candlestick_rf.joblib")
    app.state.ml_model = joblib.load(model_path) if model_path.exists() else None

    yield
    await ebest.aclose()
```

---

### 9-5. GAF-CNN 비주얼 패턴 인식

**출처:** *Encoding Candlesticks as Images for Pattern Classification Using CNNs*  
— Hasan et al., Financial Innovation (Springer), 2020 | 90.7% 정확도

OHLCV 시계열을 **Gramian Angular Field(GAF)** 이미지로 인코딩 후  
CNN으로 분류하는 방식. 규칙 기반으로 잡기 어려운 복합 패턴에 효과적.

#### GAF 인코딩 원리

```python
# services/gaf_encoder.py

import numpy as np

def encode_gaf(series: list[float], image_size: int = 24) -> np.ndarray:
    """
    1. 시계열을 [-1, 1]로 정규화
    2. arccos 변환으로 각도 φ_i 계산
    3. GAF[i,j] = cos(φ_i + φ_j)  ← 시간 관계를 2D 행렬로 인코딩
    """
    s = np.array(series[-image_size:], dtype=float)
    s_min, s_max = s.min(), s.max()
    if s_max - s_min < 1e-9:
        return np.zeros((image_size, image_size), dtype=np.float32)

    s_norm = 2 * (s - s_min) / (s_max - s_min) - 1
    s_norm = np.clip(s_norm, -1, 1)
    phi = np.arccos(s_norm)
    gaf = np.cos(phi[:, None] + phi[None, :])
    return gaf.astype(np.float32)


def candles_to_gaf_tensor(candles: list[dict], image_size: int = 24) -> np.ndarray:
    """
    OHLCV 4채널 GAF 텐서 생성. shape: (4, image_size, image_size)
    채널: [open_gaf, high_gaf, low_gaf, close_gaf]
    """
    fields = ["open", "high", "low", "close"]
    channels = [encode_gaf([c[f] for c in candles], image_size) for f in fields]
    return np.stack(channels, axis=0)
```

#### CNN 모델 구조 (PyTorch)

```python
# models/gaf_cnn.py

import torch
import torch.nn as nn

class GafCNN(nn.Module):
    """
    입력: (batch, 4채널, 24, 24) GAF 텐서
    출력: (batch, 2) — [하락 확률, 상승 확률]
    """
    def __init__(self):
        super().__init__()
        self.features = nn.Sequential(
            nn.Conv2d(4, 32, kernel_size=3, padding=1), nn.ReLU(),
            nn.Conv2d(32, 64, kernel_size=3, padding=1), nn.ReLU(),
            nn.MaxPool2d(2),                              # 12×12
            nn.Conv2d(64, 128, kernel_size=3, padding=1), nn.ReLU(),
            nn.AdaptiveAvgPool2d((4, 4)),                 # 4×4
        )
        self.classifier = nn.Sequential(
            nn.Flatten(),
            nn.Linear(128 * 4 * 4, 256), nn.ReLU(), nn.Dropout(0.3),
            nn.Linear(256, 2),
        )

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.classifier(self.features(x))
```

#### 추론 엔드포인트 추가 (`/api/patterns/{shcode}/gaf`)

```python
# routers/patterns.py 추가

@router.get("/{shcode}/gaf")
async def get_gaf_prediction(
    shcode: str,
    tf: str = Query("5m"),
    request: Request = None,
):
    """GAF-CNN 기반 상승/하락 확률 반환."""
    model = request.app.state.gaf_model   # GafCNN 인스턴스
    if model is None:
        raise HTTPException(503, "GAF model not loaded")

    token = await request.app.state.ebest.auth_token()
    candles = await request.app.state.fetcher.fetch(token, shcode, Timeframe(tf))

    tensor = candles_to_gaf_tensor(candles, image_size=24)
    x = torch.tensor(tensor).unsqueeze(0)   # (1, 4, 24, 24)
    with torch.no_grad():
        probs = torch.softmax(model(x), dim=-1)[0].tolist()

    return {"shcode": shcode, "timeframe": tf, "bearish_prob": probs[0], "bullish_prob": probs[1]}
```

---

### 9-6. 부트스트랩 통계 검증 (Bootstrap Statistical Validation)

**출처:** PLOS ONE 2020 — *Improving stock trading decisions based on pattern recognition using ML*;  
표준 방법론: FDR(False Discovery Rate) 교정으로 다중 패턴 검정 시 허위 발견 억제

#### 순열 검정 (Permutation Test)

```python
# services/backtest_engine.py 추가

def permutation_test(
    candles: list[dict],
    pattern_name: str,
    tf: Timeframe,
    hold_bars: int,
    n_permutations: int = 1000,
) -> dict:
    """
    귀무가설: 패턴 시그널은 무작위 진입과 수익률 차이가 없다.
    → 랜덤 진입 수익률 분포 생성 후 실제 전략 수익률의 p-value 계산.
    """
    real_result  = run_backtest(candles, pattern_name, tf, hold_bars)
    real_avg_ret = real_result["avg_return"]
    n_signals    = real_result["signals"]

    if n_signals < 5:
        return {**real_result, "p_value": None, "significant": False}

    closes = [c["close"] for c in candles]
    random_returns = []
    rng = np.random.default_rng(seed=42)

    for _ in range(n_permutations):
        # 같은 수의 랜덤 진입점 샘플링
        entries = rng.choice(len(closes) - hold_bars, size=n_signals, replace=False)
        rets = [(closes[i + hold_bars] - closes[i]) / closes[i] * 100 for i in entries]
        random_returns.append(np.mean(rets))

    random_returns = np.array(random_returns)
    p_value = float(np.mean(random_returns >= real_avg_ret))  # 단측 검정

    return {
        **real_result,
        "p_value":     p_value,
        "significant": p_value < 0.05,
        "random_mean": float(random_returns.mean()),
        "random_std":  float(random_returns.std()),
    }


def fdr_correction(p_values: list[float], alpha: float = 0.05) -> list[bool]:
    """
    Benjamini-Hochberg FDR 교정.
    8개 패턴 동시 검정 시 허위 발견을 억제한다.
    """
    n = len(p_values)
    sorted_idx = np.argsort(p_values)
    sorted_p   = np.array(p_values)[sorted_idx]
    threshold  = (np.arange(1, n + 1) / n) * alpha
    reject     = sorted_p <= threshold
    # 연속성 유지: reject[k]=True이면 k 이하 모두 reject
    max_reject = np.max(np.where(reject)[0]) if reject.any() else -1
    result_sorted = np.arange(n) <= max_reject
    result = np.empty(n, dtype=bool)
    result[sorted_idx] = result_sorted
    return result.tolist()
```

#### 백테스트 API 확장 (`/api/backtest` POST body 추가)

```
Body 추가 필드:
  permutation_test : bool  (기본: false) — 순열 검정 실행 여부
  n_permutations   : int   (기본: 1000)  — 순열 횟수
  fdr_correction   : bool  (기본: true)  — FDR 교정 여부

Response 추가 필드:
  by_pattern[]:
    p_value       : float | null
    significant   : bool          — FDR 교정 후 유의성
    random_mean   : float         — 랜덤 진입 평균 수익률
```

---

### 9-7. SvelteKit 최신 방법론 UI 통합

#### CompositeScoreBar 컴포넌트

```svelte
<!-- src/lib/components/CompositeScoreBar.svelte -->
<script lang="ts">
  export let confidence:  number;   // 규칙 기반 신뢰도 (Caginalp)
  export let mtfScore:    number;   // MTF 컨플루언스
  export let mlScore:     number;   // ML 앙상블
  export let volConfirmed: boolean;
  export let atrNorm:     boolean;

  $: composite = confidence * 0.3 + mtfScore * 0.3 + mlScore * 0.4;
  $: color = composite >= 0.7 ? '#a6e3a1' : composite >= 0.5 ? '#f9e2af' : '#f38ba8';
</script>

<div class="score-card">
  <div class="bar-row">
    <span>규칙(Caginalp)</span>
    <div class="bar"><div style="width:{confidence*100}%; background:#89b4fa"/></div>
    <span>{(confidence*100).toFixed(0)}%</span>
  </div>
  <div class="bar-row">
    <span>MTF 컨플루언스</span>
    <div class="bar"><div style="width:{mtfScore*100}%; background:#cba6f7"/></div>
    <span>{(mtfScore*100).toFixed(0)}%</span>
  </div>
  <div class="bar-row">
    <span>ML 앙상블</span>
    <div class="bar"><div style="width:{mlScore*100}%; background:#fab387"/></div>
    <span>{(mlScore*100).toFixed(0)}%</span>
  </div>
  <div class="composite" style="color:{color}">
    종합: {(composite*100).toFixed(0)}%
    {#if volConfirmed}<span class="badge">거래량✓</span>{/if}
    {#if atrNorm}<span class="badge">ATR✓</span>{/if}
  </div>
</div>

<style>
  .score-card { padding: 12px; background: #181825; border-radius: 8px; }
  .bar-row { display: flex; align-items: center; gap: 8px; margin: 4px 0; font-size: 12px; }
  .bar { flex: 1; height: 8px; background: #313244; border-radius: 4px; overflow: hidden; }
  .bar div { height: 100%; border-radius: 4px; transition: width 0.3s; }
  .composite { font-weight: bold; margin-top: 8px; font-size: 14px; }
  .badge { background: #313244; padding: 1px 6px; border-radius: 10px; font-size: 10px; margin-left: 4px; }
</style>
```

---

### 9-8. 최신 방법론 프롬프트 템플릿

#### [프롬프트 E] ATR 정규화 + 볼륨 확인 구현

```
`app/services/pattern_detector.py`에 다음 두 함수를 추가하라.

## _compute_atr(candles, period=14) -> float
- True Range = max(high-low, |high-prev_close|, |low-prev_close|)
- 최근 period개 TR의 단순 평균 반환
- candles 부족 시 0.0 반환

## _check_atr_threshold(candles, atr, min_body_atr_ratio=0.3) -> bool
- candles[-1].body >= atr * min_body_atr_ratio 이면 True
- atr <= 0 이면 False

## _check_volume_spike(candles, period=20, threshold=1.5) -> bool
- candles[-1].volume >= mean(candles[-(period+1):-1].volume) * threshold

## 적용 위치
PatternDetector.scan() 에서 use_modern=True 시
각 PatternResult에 atr_normalized, volume_confirmed 필드 채움.
규칙:
- numpy 사용 허용
- 타입 힌트 필수, 주석 없음
- 0 나눔 방지: `or 1e-9`
```

---

#### [프롬프트 F] ML 앙상블 모델 학습 스크립트 구현

```
`scripts/train_ml_model.py`를 구현하라.

## 데이터 생성
- EBestService + CandleFetcher로 일봉 데이터 수집
- 입력 주식 코드 리스트를 --shcodes 인자로 받음
- _extract_ml_features(candles, lookback=10) 로 피처 벡터 생성 (15개 피처)
- hold_bars=5일 후 수익률 > 0이면 label=1, 아니면 0

## 모델
- sklearn Pipeline: StandardScaler + CalibratedClassifierCV(RandomForestClassifier)
- n_estimators=200, max_depth=8, random_state=42
- 5-fold 교차 검증으로 AUC-ROC 출력

## 저장
- models/candlestick_rf.joblib 에 저장
- 저장 전 models/ 디렉토리 없으면 생성

## 실행 예시
python scripts/train_ml_model.py --shcodes 005930 035420 000660
```

---

#### [프롬프트 G] 부트스트랩 검정 + FDR 교정 백테스트 엔드포인트

```
`app/routers/backtest.py`의 POST /api/backtest 엔드포인트를 수정하라.

## 추가 요청 바디 필드
- permutation_test: bool = False
- n_permutations: int = 1000
- fdr_correction: bool = True

## 로직
1. 기존 run_backtest() 실행 (기존 구현 유지)
2. permutation_test=True 이면 permutation_test() 함수 실행
   - n_signals < 5 이면 p_value=None, significant=False 반환
3. fdr_correction=True 이고 multiple patterns 검정 시
   - 각 패턴의 p_value를 수집
   - fdr_correction(p_values) 로 유의성 재계산
   - by_pattern 각 항목에 significant 필드 업데이트

## 응답 추가 필드
by_pattern[]:
  p_value: float | null
  significant: bool
  random_mean: float
  random_std: float
```

---

### 9-9. 방법론 선택 가이드

```
용도                    권장 방법                          이유
──────────────────────────────────────────────────────────────────────────
단순 스캔·알림          Caginalp 8패턴 (규칙 기반)          속도 우선, 추가 모델 불필요
고신뢰 진입 시그널      Caginalp + ATR정규화 + 볼륨확인     구현 간단, 성능 즉시 향상
스윙 트레이딩           + MTF 컨플루언스 (5m→15m→1h)       타임프레임 정렬 시 승률 상승
퀀트 연구·검증          + 부트스트랩/FDR 검정               허위 패턴 발견 억제
자동 트레이딩 시스템    + ML 앙상블 스코어                  비선형 패턴 포착, 확률적 포지션 관리
연구·논문 재현          GAF-CNN                             시각적 패턴 90.7% 정확도 재현
```

> ⚠️ **현실 주의:** GAF-CNN(9-5)과 ML 앙상블(9-4)은 **충분한 학습 데이터(종목당 3년+ 일봉, 수천 샘플)와
> walk-forward 검증**이 없으면 과최적화로 실전 손실을 낸다. MVP에서는 **규칙 기반 + ATR + 볼륨 + MTF**
> (모두 학습 불필요, 결정론적)만으로 시작하고, ML은 Phase 4에서 백테스트로 우위가 입증된 뒤 편입하라.

---

## 10. 자동매매 엔진

> **이 섹션이 "분석 도구"를 "자동매매 시스템"으로 만든다.**  
> `ebest_service.py`의 `place_order`(CSPAT00601), `get_account_balance`(t0424)를 실제로 사용한다.

### 10-1. 전략 혼합/선택 설정 (StrategyConfig)

고정 가중치(`0.3/0.3/0.4`)를 제거하고, **사용자가 시그널 소스를 켜고/끄고/가중치를 조정**할 수 있게 한다.
이것이 "여러 방식을 혼합 또는 선택" 요구의 핵심이다.

```python
# services/strategy.py
from dataclasses import dataclass, field
from enum import Enum

class SignalSource(str, Enum):
    RULE   = "rule"     # Caginalp 규칙 신뢰도 (confidence)
    ATR    = "atr"      # ATR 적응형 임계값 통과 (0/1)
    VOLUME = "volume"   # 거래량 확인 (0/1)
    MTF    = "mtf"      # 멀티타임프레임 컨플루언스
    ML     = "ml"       # ML 앙상블 확률
    GAF    = "gaf"      # GAF-CNN 확률

@dataclass
class StrategyConfig:
    """전략 혼합/선택 — 프리셋 또는 사용자 커스텀."""
    name: str = "balanced"
    # 각 소스별 가중치 (0이면 비활성). 합이 1일 필요는 없음 — 내부에서 정규화.
    weights: dict[SignalSource, float] = field(default_factory=lambda: {
        SignalSource.RULE:   0.40,
        SignalSource.ATR:    0.15,
        SignalSource.VOLUME: 0.15,
        SignalSource.MTF:    0.30,
        SignalSource.ML:     0.00,   # 기본 비활성 (학습 모델 필요)
        SignalSource.GAF:    0.00,
    })
    enabled_patterns: list[str] = field(default_factory=lambda: [
        "three_white_soldiers", "morning_star", "bullish_engulfing", "hammer",
    ])  # 자동매매는 기본적으로 매수(상승) 패턴만 — 공매도 미사용 가정
    entry_threshold: float = 0.65    # composite_score 이 값 이상이면 진입 후보
    direction: str = "long_only"     # "long_only" | "long_short"

    def composite(self, signals: dict[SignalSource, float]) -> float:
        """활성 소스의 가중 평균. 비활성(weight=0)은 자동 제외."""
        active = {s: w for s, w in self.weights.items() if w > 0}
        total_w = sum(active.values()) or 1e-9
        return sum(signals.get(s, 0.0) * w for s, w in active.items()) / total_w


# 즉시 사용 가능한 프리셋 — UI에서 선택
STRATEGY_PRESETS: dict[str, StrategyConfig] = {
    # 보수적: 규칙+볼륨+MTF 모두 정렬돼야 진입 (신호 적지만 승률↑)
    "conservative": StrategyConfig(
        name="conservative",
        weights={SignalSource.RULE: 0.35, SignalSource.ATR: 0.15,
                 SignalSource.VOLUME: 0.20, SignalSource.MTF: 0.30,
                 SignalSource.ML: 0.0, SignalSource.GAF: 0.0},
        entry_threshold=0.75,
    ),
    # 균형 (기본값)
    "balanced": StrategyConfig(name="balanced"),
    # 공격적: 규칙 위주, 빠른 진입 (신호 많지만 노이즈↑)
    "aggressive": StrategyConfig(
        name="aggressive",
        weights={SignalSource.RULE: 0.60, SignalSource.ATR: 0.10,
                 SignalSource.VOLUME: 0.30, SignalSource.MTF: 0.0,
                 SignalSource.ML: 0.0, SignalSource.GAF: 0.0},
        entry_threshold=0.55,
    ),
    # ML 혼합: 백테스트로 검증된 경우에만 사용
    "ml_blended": StrategyConfig(
        name="ml_blended",
        weights={SignalSource.RULE: 0.25, SignalSource.ATR: 0.10,
                 SignalSource.VOLUME: 0.10, SignalSource.MTF: 0.20,
                 SignalSource.ML: 0.35, SignalSource.GAF: 0.0},
        entry_threshold=0.62,
    ),
}
```

**`PatternResult.composite_score`는 더 이상 하드코딩하지 않는다.** `StrategyConfig.composite()`로 계산:

```python
# pattern_detector.py 의 scan() 내부 수정 (9-x의 고정 가중치 대체)
def _apply_strategy(result: PatternResult, cfg: StrategyConfig) -> None:
    signals = {
        SignalSource.RULE:   result.confidence,
        SignalSource.ATR:    1.0 if result.atr_normalized else 0.0,
        SignalSource.VOLUME: 1.0 if result.volume_confirmed else 0.0,
        SignalSource.MTF:    result.mtf_score,
        SignalSource.ML:     result.ml_score,
        SignalSource.GAF:    result.gaf_score,
    }
    result.composite_score = cfg.composite(signals)
```

---

### 10-2. 페이퍼 트레이딩 vs 실전 (TradingMode)

> 실전 직행은 금물. **동일한 코드 경로**로 모의/실전을 전환해 검증 후 투입한다.

```python
# services/broker.py
from enum import Enum
import logging

logger = logging.getLogger(__name__)

class TradingMode(str, Enum):
    PAPER = "paper"   # 체결 시뮬레이션 (실주문 X)
    LIVE  = "live"    # 실제 eBest 주문

class Broker:
    """주문 실행 추상화. PAPER/LIVE 동일 인터페이스."""
    def __init__(self, ebest: EBestService, mode: TradingMode = TradingMode.PAPER):
        self._ebest = ebest
        self._mode = mode
        self._paper_positions: dict[str, dict] = {}   # PAPER 모드 가상 포지션
        self._paper_cash: float = 10_000_000           # 가상 시드 1천만원

    async def buy(self, token: str, code: str, qty: int, price: float) -> dict:
        if self._mode == TradingMode.PAPER:
            cost = qty * price
            if cost > self._paper_cash:
                return {"ok": False, "reason": "insufficient_paper_cash"}
            self._paper_cash -= cost
            self._paper_positions[code] = {"qty": qty, "avg_price": price}
            logger.info("[PAPER] BUY %s x%d @%.0f", code, qty, price)
            return {"ok": True, "mode": "paper", "code": code, "qty": qty, "price": price}
        # LIVE: 지정가 매수
        res = await self._ebest.place_order(token, code, "buy", qty, price, price_type="00")
        return {"ok": res.get("rsp_cd") == "0000", "mode": "live", "raw": res}

    async def sell(self, token: str, code: str, qty: int, price: float) -> dict:
        if self._mode == TradingMode.PAPER:
            pos = self._paper_positions.pop(code, None)
            if not pos:
                return {"ok": False, "reason": "no_paper_position"}
            self._paper_cash += qty * price
            logger.info("[PAPER] SELL %s x%d @%.0f", code, qty, price)
            return {"ok": True, "mode": "paper", "code": code, "qty": qty, "price": price}
        res = await self._ebest.place_order(token, code, "sell", qty, price, price_type="00")
        return {"ok": res.get("rsp_cd") == "0000", "mode": "live", "raw": res}
```

---

### 10-3. 리스크 관리 (RiskManager) — 진입 전 필수 게이트

> **모든 진입은 리스크 게이트를 통과해야 한다.** 이것이 계좌를 지킨다.

```python
# services/risk.py
from dataclasses import dataclass

@dataclass
class RiskConfig:
    max_position_pct:    float = 0.10   # 종목당 최대 자본 비중 10%
    max_positions:       int   = 5      # 동시 보유 종목 수
    risk_per_trade_pct:  float = 0.01   # 1회 거래 최대 손실 = 자본의 1%
    stop_loss_atr_mult:  float = 1.5    # 손절 = 진입가 - 1.5×ATR
    take_profit_atr_mult: float = 3.0   # 익절 = 진입가 + 3.0×ATR (R:R = 1:2)
    daily_loss_limit_pct: float = 0.03  # 일일 누적 손실 3% 도달 시 당일 매매 중단
    trailing_stop_atr:   float = 2.0    # 트레일링 스톱 (수익 보호)

class RiskManager:
    def __init__(self, cfg: RiskConfig):
        self.cfg = cfg
        self._daily_pnl: float = 0.0
        self._open_positions: dict[str, dict] = {}

    def can_enter(self, equity: float) -> tuple[bool, str]:
        if self._daily_pnl <= -equity * self.cfg.daily_loss_limit_pct:
            return False, "daily_loss_limit_reached"
        if len(self._open_positions) >= self.cfg.max_positions:
            return False, "max_positions_reached"
        return True, "ok"

    def position_size(self, equity: float, entry: float, atr: float) -> int:
        """변동성 기반 포지션 사이징 (ATR 손절폭으로 1R 위험 고정)."""
        stop_distance = atr * self.cfg.stop_loss_atr_mult
        if stop_distance <= 0:
            return 0
        risk_amount = equity * self.cfg.risk_per_trade_pct
        qty_by_risk = int(risk_amount / stop_distance)        # 손실폭 기준 수량
        qty_by_cap  = int(equity * self.cfg.max_position_pct / entry)  # 비중 상한
        return max(0, min(qty_by_risk, qty_by_cap))

    def stop_and_target(self, entry: float, atr: float) -> tuple[float, float]:
        stop   = entry - atr * self.cfg.stop_loss_atr_mult
        target = entry + atr * self.cfg.take_profit_atr_mult
        return stop, target

    def register(self, code: str, entry: float, qty: int, stop: float, target: float) -> None:
        self._open_positions[code] = {
            "entry": entry, "qty": qty, "stop": stop, "target": target,
            "peak": entry,
        }

    def check_exit(self, code: str, price: float, atr: float) -> str | None:
        """보유 포지션의 청산 사유 판정. None=홀드."""
        pos = self._open_positions.get(code)
        if not pos:
            return None
        pos["peak"] = max(pos["peak"], price)
        trail = pos["peak"] - atr * self.cfg.trailing_stop_atr
        if price <= pos["stop"]:
            return "stop_loss"
        if price >= pos["target"]:
            return "take_profit"
        if price <= trail and price > pos["entry"]:
            return "trailing_stop"
        return None

    def on_close(self, code: str, pnl: float) -> None:
        self._daily_pnl += pnl
        self._open_positions.pop(code, None)

    def reset_daily(self) -> None:
        self._daily_pnl = 0.0
```

---

### 10-4. 매매 오케스트레이터 (TradingEngine) — 전체 루프

```python
# services/trading_engine.py
import asyncio
from datetime import datetime, time as dtime

KST_OPEN  = dtime(9, 0)
KST_CLOSE = dtime(15, 20)   # 동시호가 전 청산 권장

class TradingEngine:
    def __init__(self, ebest, fetcher, detector, broker,
                 strategy: StrategyConfig, risk: RiskManager,
                 watchlist: list[str], tf: Timeframe = Timeframe.M5):
        self.ebest = ebest; self.fetcher = fetcher; self.detector = detector
        self.broker = broker; self.strategy = strategy; self.risk = risk
        self.watchlist = watchlist; self.tf = tf
        self._running = False

    def _market_open(self) -> bool:
        now = datetime.now(KST).time()
        return KST_OPEN <= now <= KST_CLOSE

    async def _equity(self, token: str) -> float:
        bal = await self.ebest.get_account_balance(token)
        # t0424 응답에서 예수금+평가금 합산 (필드명은 실제 응답 확인 후 매핑)
        return float(bal.get("t0424OutBlock", {}).get("sunamt", 0) or 0) or 10_000_000

    async def _scan_and_trade(self, token: str) -> None:
        equity = await self._equity(token)
        for code in self.watchlist:
            candles = await self.fetcher.fetch(token, code, self.tf)
            if len(candles) < 20:
                continue
            cobjs = [Candle(**{k: v for k, v in r.items()
                     if k in Candle.__dataclass_fields__}) for r in candles]
            atr = _compute_atr(cobjs)
            price = cobjs[-1].close

            # ── 보유 포지션 청산 체크 우선 ──
            reason = self.risk.check_exit(code, price, atr)
            if reason:
                pos = self.risk._open_positions[code]
                res = await self.broker.sell(token, code, pos["qty"], price)
                if res["ok"]:
                    pnl = (price - pos["entry"]) * pos["qty"]
                    self.risk.on_close(code, pnl)
                    logger.info("EXIT %s (%s) pnl=%.0f", code, reason, pnl)
                continue

            # ── 신규 진입 체크 ──
            results = self.detector.scan(candles, self.tf, strategy=self.strategy)
            cands = [r for r in results
                     if r.pattern_name in self.strategy.enabled_patterns
                     and r.composite_score >= self.strategy.entry_threshold]
            if not cands:
                continue
            ok, why = self.risk.can_enter(equity)
            if not ok:
                logger.info("entry blocked: %s", why); continue
            qty = self.risk.position_size(equity, price, atr)
            if qty <= 0:
                continue
            res = await self.broker.buy(token, code, qty, price)
            if res["ok"]:
                stop, target = self.risk.stop_and_target(price, atr)
                self.risk.register(code, price, qty, stop, target)
                logger.info("ENTER %s x%d @%.0f stop=%.0f target=%.0f",
                            code, qty, price, stop, target)

    async def run(self, poll_sec: int = 60) -> None:
        """장중 폴링 루프. tf 봉 주기에 맞춰 poll_sec 조정 (5분봉이면 60~300)."""
        self._running = True
        self.risk.reset_daily()
        token = await self.ebest.auth_token()
        while self._running:
            if self._market_open():
                try:
                    await self._scan_and_trade(token)
                except Exception as exc:
                    logger.error("trade loop error: %s", exc)
            await asyncio.sleep(poll_sec)

    def stop(self) -> None:
        self._running = False
```

---

### 10-5. 자동매매 제어 API & UI

#### API 엔드포인트

```
POST /api/trading/start
  Body: { mode: "paper"|"live", strategy: "conservative"|"balanced"|"aggressive"|"ml_blended"|custom,
          watchlist: string[], tf: Timeframe, risk: RiskConfig }
  → 백그라운드 TradingEngine.run() 기동

POST /api/trading/stop          → 엔진 정지 (보유 포지션은 유지/청산 옵션)
GET  /api/trading/status        → { mode, running, positions[], daily_pnl, equity }
GET  /api/trading/positions     → 현재 보유 포지션 + 손절/익절가 + 평가손익
PUT  /api/trading/strategy      → 실행 중 전략 가중치 핫스왑
```

#### 자동매매 대시보드 (SvelteKit `routes/trading/+page.svelte`)

```
┌─ 자동매매 컨트롤 ───────────────────────────────────┐
│  [모드: ●페이퍼 ○실전]   [전략: balanced ▼]         │
│  관심종목: 005930 035420 000660 ... (+추가)         │
│  타임프레임: [5분 ▼]    [▶ 시작]  [■ 정지]          │
├─ 전략 가중치 (실시간 조정 슬라이더) ─────────────────┤
│  규칙   ████████░░ 0.40    ATR    ███░░░░░ 0.15      │
│  볼륨   ███░░░░░ 0.15      MTF    ██████░ 0.30       │
│  ML     ░░░░░░░ 0.00 (모델 없음)                     │
│  진입 임계: ●━━━━━━━ 0.65                            │
├─ 리스크 한도 ───────────────────────────────────────┤
│  종목당 10% │ 최대 5종목 │ 1R=1% │ 일손실 -3% 정지   │
├─ 현재 포지션 ───────────────────────────────────────┤
│  005930  10주  진입 73,400  현재 74,100  +0.95%      │
│          손절 72,300  익절 75,600  [수동청산]        │
└─ 일일 손익: +1.2%  │  당일 거래: 3건 (2승 1패) ─────┘
```

---

## 11. 현실적 백테스트 & 통합 구현 프롬프트

### 11-1. Walk-Forward 검증 (과최적화 방지)

> ML/가중치 튜닝 시 **반드시** in-sample/out-of-sample을 시간순 분리한다. 무작위 split은 데이터 누수.

```python
# services/walk_forward.py
def walk_forward_split(candles: list[dict], n_folds: int = 4, train_ratio: float = 0.7):
    """
    시간순 롤링 윈도우. 각 fold는 (train, test)를 시간순으로 분리.
    절대 미래 데이터로 학습하지 않는다.
    """
    n = len(candles)
    fold_size = n // (n_folds + 1)
    for k in range(n_folds):
        train_end = fold_size * (k + 1)
        test_end  = train_end + fold_size
        yield candles[:train_end], candles[train_end:test_end]


def evaluate_strategy(candles, strategy: StrategyConfig, hold_bars: int = 5) -> dict:
    """전략 프리셋을 walk-forward로 평가. out-of-sample 성과만 집계."""
    oos_returns = []
    for train, test in walk_forward_split(candles):
        # (ML 사용 시 train으로만 학습, test로 평가)
        for pattern in strategy.enabled_patterns:
            r = run_backtest(test, pattern, Timeframe.D1, hold_bars)
            if r["signals"] > 0:
                oos_returns.append(r["avg_return"])
    return {
        "oos_avg_return": float(np.mean(oos_returns)) if oos_returns else 0.0,
        "oos_consistency": float(np.mean([x > 0 for x in oos_returns])) if oos_returns else 0.0,
    }
```

### 11-2. 실전 투입 체크리스트

```
실거래 전 반드시 확인:
□ 백테스트가 거래비용(CostModel) 차감 후에도 profit_factor > 1.3
□ Walk-forward out-of-sample 수익률이 in-sample의 50% 이상 유지
□ 페이퍼 트레이딩 2주 이상, 백테스트 대비 슬리피지 괴리 < 0.2%
□ 일일 손실 한도(-3%) 도달 시 자동 정지 동작 확인
□ 손절 주문이 미체결될 때 시장가 청산 폴백 존재
□ 토큰 만료/네트워크 단절 시 포지션 상태 복구 로직 (재시작 시 t0424로 동기화)
□ 장 마감 동시호가(15:20~) 전 데이트레이딩 포지션 청산 규칙
□ EBest rate limit 준수 — 주문 TR(CSPAT00601)은 enforce_rate_limit 적용됨
```

### 11-3. [프롬프트 H] 자동매매 엔진 통합 구현 (마스터)

```
다음 명세로 자동매매 시스템을 구현하라. 기존 pattern_detector / candle_fetcher 를 재사용한다.

## 구현 파일
1. services/strategy.py    — SignalSource Enum, StrategyConfig, STRATEGY_PRESETS, _apply_strategy()
2. services/broker.py      — TradingMode Enum, Broker (paper/live 동일 인터페이스)
3. services/risk.py        — RiskConfig, RiskManager (사이징/손절/익절/일손실한도/트레일링)
4. services/trading_engine.py — TradingEngine (장중 폴링 루프, 청산 우선 → 진입)
5. routers/trading.py      — start/stop/status/positions/strategy API

## 핵심 규칙 (현실성 필수)
- 진입은 항상 '다음 봉 시가' 가정 (백테스트와 라이브 일치)
- 모든 진입 전 RiskManager.can_enter() + position_size() 통과 필수
- 청산 체크(check_exit)를 신규 진입보다 먼저 실행
- PAPER 모드는 실제 place_order 호출 금지, 가상 포지션만 갱신
- LIVE 모드는 ebest.place_order(CSPAT00601) 사용, rsp_cd=="0000" 성공 판정
- 장 운영시간(09:00~15:20 KST) 밖에서는 신규 진입 금지
- 일일 손실 -3% 도달 시 당일 신규 진입 중단

## StrategyConfig 가중치는 절대 하드코딩하지 말 것
- composite_score = StrategyConfig.composite(signals) 로만 계산
- 비활성 소스(weight=0)는 자동 제외 후 정규화

## 안전장치
- 손절 지정가 미체결 시 시장가(price_type="03") 폴백
- 엔진 재시작 시 get_account_balance(t0424)로 실제 보유와 동기화
- 모든 주문/청산은 logger.info로 audit log 남김

## 비기능 요구
- TradingEngine.run()은 asyncio 백그라운드 태스크 (FastAPI BackgroundTasks 또는 app.state에 보관)
- 타입 힌트 필수, 예외는 루프를 죽이지 않고 logger.error 후 계속
```

### 11-4. [프롬프트 I] 전략 프리셋 백테스트 비교 도구

```
`scripts/compare_strategies.py`를 구현하라.

## 목적
STRATEGY_PRESETS 의 모든 프리셋을 동일 종목/기간에 walk-forward 백테스트하여
거래비용 차감 후 성과를 표로 비교 출력.

## 출력 컬럼
preset | signals | win_rate | avg_return(net) | profit_factor | max_dd | oos_consistency

## 규칙
- run_backtest()에 CostModel() 적용 (거래비용 필수 반영)
- walk_forward_split()으로 out-of-sample만 집계
- profit_factor 내림차순 정렬, >1.3 인 프리셋에 ★ 표시
- 실행: python scripts/compare_strategies.py --shcodes 005930 035420 --tf 1d --hold 5
```

### 11-5. 권장 운영 아키텍처 (최종)

```
┌──────────────┐   장중 폴링(60s)   ┌─────────────────┐
│ TradingEngine │──────────────────▶│ 관심종목 ≤30개   │  ← 실시간 자동매매
│  (paper/live) │   청산>진입 순서   │ 5분봉 스캔       │
└──────┬───────┘                    └─────────────────┘
       │ 주문/청산
       ▼
┌──────────────┐                    ┌─────────────────┐
│ Broker        │   CSPAT00601      │ RiskManager      │
│ (eBest)       │◀──게이트 통과만──▶│ 사이징/손절/한도 │
└──────────────┘                    └─────────────────┘

┌──────────────┐  장 마감 후 16:00  ┌─────────────────┐
│ 배치 스캐너   │──────────────────▶│ 전종목 일봉      │  ← 다음날 관심종목 선정
│ (스케줄러)    │   Semaphore(5)     │ 패턴+ML 스코어   │
└──────────────┘                    └─────────────────┘
```

---

*기반 논문: Caginalp & Laurent (1998), Journal of Applied Mathematics and Stochastic Analysis, 11(3)*  
*최신 확장: Financial Innovation (2020), PLOS ONE (2020), IEEE Xplore (2022), arXiv:2201.08669 (2022), IJRPR (2024)*  
*자동매매 설계: 거래비용 모델 + 변동성 기반 포지션 사이징 + Walk-forward 검증 (실전 리스크 관리 표준)*
