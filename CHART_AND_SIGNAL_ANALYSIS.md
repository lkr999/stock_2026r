# 차트 데이터 출처 & 매수·매도 시그널 로직 분석

> 대상 시스템: Caginalp & Laurent (1998) 캔들스틱 자동매매 (FastAPI + SvelteKit + eBest)
> 본 문서는 실제 구현 코드(`backend/app/services/*`)를 기준으로 작성됨.

---

## 목차

1. [차트 데이터는 어디서 오는가](#1-차트-데이터는-어디서-오는가)
2. [매수·매도 시그널 전체 흐름](#2-매수매도-시그널-전체-흐름)
3. [패턴 감지 & 신뢰도 계산](#3-패턴-감지--신뢰도-계산)
4. [보조 필터 (ATR / 거래량 / MTF / ML)](#4-보조-필터)
5. [종합 점수(composite) 계산](#5-종합-점수composite-계산)
6. [진입(매수) 조건](#6-진입매수-조건)
7. [청산(매도) 조건](#7-청산매도-조건)
8. [전략별 상세 정리](#8-전략별-상세-정리)
9. [모의투자 vs 실전투자](#9-모의투자-vs-실전투자)

---

## 1. 차트 데이터는 어디서 오는가

### 1-1. 데이터 흐름도

```
[SvelteKit 차트]                [FastAPI]                    [데이터 소스]
                ┌──────────────────────────────────────┐
 CandleChart  ──┤ GET /api/candles/{code}              ├──┐
 TradingChart ──┤ GET /api/candles/{code}/indicators   ├──┤
                └──────────────────────────────────────┘  │
                                  │                         │
                          CandleFetcher.fetch(tf)           │
                                  │                         │
                    ┌─────────────┴──────────────┐          │
            tf == 일봉(1d)              tf == 분봉(1m~60m)   │
                    │                            │          │
        ebest.fetch_daily_candles      ebest.fetch_candle_data_tr
              (TR: t8410)                    (TR: t8412)     │
                    │                            │          │
                    └──────────── eBest REST API ┘          │
                                                            │
   ※ MOCK_MODE=true 또는 EBEST 키 없음 → SyntheticFetcher (합성 캔들)
```

### 1-2. 핵심 코드 위치

| 데이터 | 출처 | 코드 |
|--------|------|------|
| **일봉 캔들** | eBest TR `t8410` | `ebest_service.fetch_daily_candles()` |
| **분봉 캔들** (1·3·5·10·15·30·60분) | eBest TR `t8412` | `ebest_service.fetch_candle_data_tr(ncnt=…)` |
| **타임프레임 라우팅** | — | `candle_fetcher.CandleFetcher.fetch()` |
| **오프라인/MOCK 데이터** | 합성(랜덤워크+패턴 주입) | `synthetic.SyntheticFetcher` |
| **종목 목록·현재가·이름** | `files/search_item.csv` | `universe.load_universe()` |
| **보조지표** | 캔들에서 계산 | `indicators.compute_all()` |

### 1-3. 타임프레임 ↔ eBest TR 매핑 (`timeframe.py`)

```
타임프레임   TR      ncnt   조회봉수   추세판단봉
 1m         t8412    1      500        20
 5m(기본)   t8412    5      300        12
15m         t8412   15      200        10
60m         t8412   60      100         8
 1d         t8410    -       60         5
```

### 1-4. 캔들 데이터 형태 (통일된 OHLCV)

eBest 응답(t8410/t8412)을 `CandleFetcher`가 아래 형태로 정규화하여 반환:

```json
{ "date":"20260618", "time":"093000", "ts":"20260618 093000",
  "open":73400, "high":73800, "low":73200, "close":73600, "volume":12345 }
```

- 일봉: `t8410OutBlock1`, 거래량 = `jdiff_vol`
- 분봉: `t8412OutBlock1`, `ts = date+time` 으로 오름차순 정렬

### 1-5. 보조지표는 별도 데이터가 아니라 "캔들에서 계산"

`/api/candles/{code}/indicators` 호출 시 동일한 캔들에서 `indicators.py`가 계산:

| 지표 | 계산식 | 함수 |
|------|--------|------|
| MA5 / MA20 / MA60 | 종가 단순이동평균 | `sma()` |
| 볼린저밴드(20,2σ) | 중심=MA20, 상/하단=MA20±2×표준편차 | `bollinger()` |
| RSI(14) | Wilder 평활화 RS → 100−100/(1+RS) | `rsi()` |
| MACD(12,26,9) | EMA12−EMA26, 시그널=MACD의 EMA9, 히스토그램=MACD−시그널 | `macd()` |

> ⚠️ **중요:** 보조지표는 매매 시그널에 **직접 사용되지 않는다**. 시각적 참고용이며,
> 실제 매수/매도 판단은 §3~§7의 캔들 패턴 + 보조 필터 + 종합 점수로 이루어진다.

---

## 2. 매수·매도 시그널 전체 흐름

자동매매 엔진(`trading_engine.TradingEngine._scan_and_trade`)은 워치리스트의 각 종목에 대해
폴링 주기마다 아래 순서로 동작한다. **청산을 진입보다 먼저** 확인한다.

```
종목별 루프 (워치리스트):
  1) 캔들 조회 → ATR 계산 → 현재가(price = 마지막 봉 종가)

  2) [청산 우선] 보유 중이면 risk.check_exit(price, atr)
        → stop_loss / take_profit / trailing_stop 중 하나면 매도(broker.sell)
        → 매도 체결 시 현금 +, 포지션 종료, 저널 기록

  3) 이미 보유 중이면 신규 진입 건너뜀

  4) [진입 판정] detector.scan(candles, strategy)  → 패턴 + 종합점수
        조건 A: pattern_name ∈ 전략.enabled_patterns
        조건 B: composite_score ≥ 전략.entry_threshold
        조건 C: risk.can_enter(equity)  (일손실 한도·최대 종목수 미달)
        조건 D: qty = risk.position_size(...) > 0  &  qty×price ≤ 현금
        → 모두 충족 시 매수(broker.buy) → 손절·익절가 등록
```

핵심: **시그널 = "패턴 발생" + "종합점수 ≥ 임계값" + "리스크 게이트 통과"** 의 3중 조건.

---

## 3. 패턴 감지 & 신뢰도 계산

### 3-1. 캔들 패턴 (Caginalp & Laurent 1998 + 단타 보강)

이 시스템은 **매수 전용(롱 전용)** 이다. 하락 반전 패턴(삼흑병·석별형·교수형·하락장악 등)은
진입 신호가 될 수 없으므로 **탐지기 자체를 두지 않는다**. 감지되는 패턴은 모두 상승 패턴이다.

| # | 패턴 | 봉수 | 추세 전제 |
|---|------|------|-----------|
| 1 | three_white_soldiers (삼백병) | 3 | 하락추세 후 |
| 2 | morning_star (샛별형) | 3 | 하락추세 후 |
| 3 | hammer (망치형) | 1 | 하락추세 후 |
| 4 | bullish_engulfing (상승장악) | 2 | 하락추세 후 |
| 5 | pin_bar_bull (상승 핀바) | 1 | — |
| 6 | inside_bar_break_up (인사이드바 상향돌파) | 3 | — |
| 7 | tweezer_bottom (집게바닥) | 2 | 하락추세 후 |
| 8 | marubozu_bull (양봉 마루보주) | 1 | — |

- **추세 판단**: 패턴 직전 N봉(타임프레임별 `trend_lookback`)의 종가 **선형회귀 기울기** 부호
  (`trend_slope`). 상승반전 패턴은 기울기 < 0(하락추세)일 때만 성립한다.

### 3-2. 신뢰도(confidence) 계산식 — 패턴별 (모두 0~1 클램핑)

| 패턴 | confidence 공식 |
|------|-----------------|
| three_white_soldiers | `min(몸통비율)/0.60×0.5 + (c3종가−c1시가)/평균몸통/3×0.5` |
| morning_star | `recovery×0.6 + c1몸통비율×0.4` (recovery=(c3종가−c2종가)/c1몸통) |
| hammer | `(아래꼬리/(몸통×2))×0.7 + 몸통위치×0.3` |
| bullish_engulfing | `(장악비율−1)×0.5 + 0.5` (장악비율=c2몸통/c1몸통) |
| pin_bar_bull | `min(아래꼬리/전체범위, 1)×0.7 + 0.3` |
| inside_bar_break_up | `min(0.55 + (돌파종가−모봉고가)/모봉범위×0.45, 1)` |
| tweezer_bottom | `min(0.6 + c2몸통비율×0.4, 1)` |
| marubozu_bull | `몸통비율` (≥0.90 일 때만 성립) |

- 몸통비율 = `|종가−시가| / (고가−저가)`
- 각 패턴은 형태 조건(몸통비율 ≥ 0.6, 갭, 꼬리 길이 등)을 먼저 통과해야 하며,
  통과 시에만 confidence를 산출한다. 미통과면 `None`(시그널 없음).

### 3-3. 기대수익(expected_5/10/25)

각 패턴은 논문 Table 3의 5/10/25봉 후 평균 초과수익률을 그대로 부착(`PAPER_RETURNS`).
예: 상승장악 +1.12%/+1.56%/+2.34%. **판단에는 쓰이지 않고 참고 표시용.**

---

## 4. 보조 필터

`PatternDetector.scan(use_modern=True)` 일 때 패턴마다 아래 보조 신호를 부착한다.

| 신호 | 의미 | 판정식 (`pattern_detector.py`) |
|------|------|--------------------------------|
| **ATR 정규화** (`atr_normalized`) | 변동성 대비 충분한 몸통인가 | `마지막봉 몸통 ≥ ATR(14) × 0.3` |
| **거래량 확인** (`volume_confirmed`) | 거래량 동반 돌파인가 | `마지막봉 거래량 ≥ 직전 N봉 평균 × 1.5`<br>(N=10 분봉 / 20 일봉) |
| **MTF 컨플루언스** (`mtf_score`) | 상위 타임프레임도 같은 방향인가 | 상위 TF에서 동일방향 패턴 비율 (0~1) |
| **ML 점수** (`ml_score`) | 학습 모델 상승확률 | RandomForest `predict_proba` (모델 있을 때만) |

- ATR = 최근 14봉 True Range 평균 (`compute_atr`).
- MTF는 `/api/patterns?mtf=true` 또는 전략에 MTF 가중치가 있을 때 계산.
- ML/GAF는 학습 모델이 주입된 경우에만 동작(기본 0).

---

## 5. 종합 점수(composite) 계산

`StrategyConfig.composite()` — **활성 소스(가중치>0)의 가중평균, 정규화**:

```
composite_score = Σ(signalᵢ × weightᵢ) / Σ(weightᵢ)     (가중치 0인 소스는 제외)

signal 값:
  RULE   = confidence (패턴 신뢰도, 0~1)
  ATR    = 1.0 if atr_normalized else 0.0
  VOLUME = 1.0 if volume_confirmed else 0.0
  MTF    = mtf_score (0~1)
  ML     = ml_score (0~1)
  GAF    = gaf_score (0~1)
```

> 즉 **종합점수는 "패턴 신뢰도"에 "보조 필터 통과 여부"를 전략 가중치로 섞은 값**이다.
> 가중치 구성이 곧 전략의 정체성이며, §8에서 전략별로 정리한다.

**예시 (balanced 전략, 가중치 RULE0.40·ATR0.15·VOL0.15·MTF0.30):**
- 상승장악 confidence=0.8, ATR통과, 거래량통과, MTF=0.5 라면
- `(0.8×0.40 + 1×0.15 + 1×0.15 + 0.5×0.30) / (0.40+0.15+0.15+0.30)`
- `= (0.32+0.15+0.15+0.15) / 1.0 = 0.77` → 임계값 0.65 초과 → **매수 후보**

---

## 6. 진입(매수) 조건

`trading_engine._scan_and_trade()` 진입부. **아래 5조건을 모두 충족해야 매수.**

| 순서 | 조건 | 코드 |
|------|------|------|
| A | 패턴이 전략의 `enabled_patterns`에 포함 | `r.pattern_name in strategy.enabled_patterns` |
| B | 종합점수 ≥ 전략 임계값 | `r.composite_score >= strategy.entry_threshold` |
| C | 리스크 게이트 통과 | `risk.can_enter(equity)` |
| D | 포지션 수량 > 0 | `qty = risk.position_size(...)` |
| E | 매수금액 ≤ 보유현금 | `qty × price ≤ cash` |

### 6-1. 리스크 게이트 (`can_enter`)

- 일일 누적손익이 **−3%(자본 대비)** 도달 시 당일 신규진입 차단
- 동시 보유 종목 수가 **최대 5종목** 도달 시 차단

### 6-2. 포지션 사이징 (`position_size`) — 변동성 기반

```
손절폭 = ATR × 1.5
1R(허용손실) = 자본 × 1%(risk_per_trade)
수량 = min( 1R / 손절폭,  자본 × 10%(max_position) / 진입가 )
```

→ **모든 거래의 최대 손실을 자본의 1%로 고정**, 종목당 비중은 10% 상한.

### 6-3. 기본 거래 대상 패턴 (매수 전용)

기본 `enabled_patterns` = `three_white_soldiers`, `morning_star`, `bullish_engulfing`,
`hammer`. 시스템 전체가 매수 전용이라 **방향 설정(direction) 자체가 없으며**, 모든 진입은
매수·모든 청산은 매도다. 사용할 패턴 목록만 전략별로 커스터마이즈한다.

---

## 7. 청산(매도) 조건

`risk.check_exit(code, price, atr)` — 보유 포지션마다 매 폴링에서 검사. **셋 중 하나라도 해당 시 매도.**

| 사유 | 조건 | 의미 |
|------|------|------|
| **stop_loss** (손절) | `price ≤ 진입가 − ATR×1.5` | 손실 제한 |
| **take_profit** (익절) | `price ≥ 진입가 + ATR×3.0` | 목표 도달 (손익비 1:2) |
| **trailing_stop** (추적손절) | `price ≤ 최고가 − ATR×2.0` 이고 `price > 진입가` | 수익 보호 |

- `최고가(peak)`는 보유 중 갱신되는 고점.
- 손익비(Risk:Reward) = 손절 1.5ATR : 익절 3.0ATR = **1 : 2**.
- 매도 체결가는 §9의 모드에 따라 다름(모의=현재가, 실전=eBest 주문).

---

## 8. 전략별 상세 정리

4개 프리셋(`strategy.STRATEGY_PRESETS`). **차이는 ① 소스 가중치, ② 진입 임계값** 뿐이며,
패턴 감지·리스크 관리·청산 로직은 모든 전략이 공통이다.

### 8-1. 가중치 & 임계값 비교표

| 소스 | conservative | balanced(기본) | aggressive | ml_blended |
|------|:---:|:---:|:---:|:---:|
| RULE (패턴신뢰도) | 0.35 | **0.40** | 0.60 | 0.25 |
| ATR (변동성) | 0.15 | 0.15 | 0.10 | 0.10 |
| VOLUME (거래량) | 0.20 | 0.15 | 0.30 | 0.10 |
| MTF (멀티TF) | 0.30 | 0.30 | 0.00 | 0.20 |
| ML (앙상블) | 0.00 | 0.00 | 0.00 | 0.35 |
| GAF (CNN) | 0.00 | 0.00 | 0.00 | 0.00 |
| **진입 임계값** | **0.75** | **0.65** | **0.55** | **0.62** |

### 8-2. conservative (보수적)

- **가중치**: 규칙 0.35 + ATR 0.15 + 거래량 0.20 + **MTF 0.30**, 임계값 **0.75(높음)**
- **성격**: 규칙·거래량·상위 타임프레임이 **모두 정렬**돼야 통과. 신호 수는 적지만 신뢰도 높음.
- **적합**: 적은 횟수로 고확률 진입을 원할 때, 변동성 큰 장에서 노이즈 회피.
- **특징**: MTF 비중이 커서 5m→15m→60m가 같은 방향일 때만 강한 점수.

### 8-3. balanced (균형, 기본값)

- **가중치**: 규칙 0.40 + ATR 0.15 + 거래량 0.15 + MTF 0.30, 임계값 **0.65**
- **성격**: 규칙과 MTF를 균형 있게 반영. 신호 빈도/신뢰도 절충.
- **적합**: 일반적 용도의 기본 전략. 처음 운용 시 권장.

### 8-4. aggressive (공격적)

- **가중치**: **규칙 0.60(높음)** + ATR 0.10 + **거래량 0.30**, **MTF 0(미사용)**, 임계값 **0.55(낮음)**
- **성격**: 단일 타임프레임의 패턴+거래량만으로 빠르게 진입. 상위 TF 확인 생략.
- **적합**: 단기·스캘핑, 신호를 많이 잡고 싶을 때. **노이즈·휩쏘 위험 증가**.
- **특징**: MTF=0이므로 분봉 신호에 가장 민감하게 반응.

### 8-5. ml_blended (ML 혼합)

- **가중치**: 규칙 0.25 + ATR 0.10 + 거래량 0.10 + MTF 0.20 + **ML 0.35**, 임계값 **0.62**
- **성격**: 규칙 신호를 ML 앙상블 확률과 결합. 비선형 패턴 포착 기대.
- **적합**: 충분한 학습 데이터로 모델을 훈련·검증한 후. **모델 미주입 시 ML=0**이 되어
  사실상 규칙·거래량·MTF만으로 동작(과신 금지).
- **⚠️ 주의**: ML/GAF는 walk-forward 검증 없이는 과최적화 위험. Phase 4 권장.

### 8-6. 전략 선택 가이드

```
원하는 것                         → 추천 전략
─────────────────────────────────────────────
적은 신호·높은 신뢰도             → conservative
무난한 기본 운용                  → balanced
많은 신호·빠른 진입(단기)         → aggressive
검증된 ML 모델 보유               → ml_blended
```

> 전략은 UI(자동매매 페이지)에서 **가중치 슬라이더로 실시간 조정** 가능하며,
> 비활성(가중치 0) 소스는 자동 제외 후 정규화된다. 즉 위 4개는 출발점일 뿐
> 사용자가 자유롭게 혼합/커스터마이즈할 수 있다.

---

## 9. 모의투자 vs 실전투자

**유일한 차이는 주문 체결 방식**이며, 시그널 감지·종합점수·리스크 관리·청산·저널 기록은 동일.

| 구분 | 모의투자(PAPER) | 실전투자(LIVE) |
|------|----------------|----------------|
| 매수 | 현재가로 즉시 체결(시뮬레이션) | eBest `place_order`(CSPAT00601) 지정가 |
| 매도 | 현재가로 즉시 체결 | eBest 주문(미체결 폴백 시 시장가 `03`) |
| 시드 자본 | 가상 시드(기본 1천만원) | 시작 시 실계좌 예수금(t0424) 조회 |
| 코드 경로 | `Broker.buy/sell` 의 PAPER 분기 | `Broker.buy/sell` 의 LIVE 분기 |

```
시그널 → 게이트 → 수량결정  ← 여기까지 두 모드 100% 동일
            │
            ▼
       Broker.buy/sell  ← 이 한 곳에서만 갈림
        ├ PAPER: 현재가 체결(시뮬)
        └ LIVE : eBest API 주문 전송
```

---

## 부록: 핵심 코드 맵

| 기능 | 파일 |
|------|------|
| 캔들 데이터 라우팅 | `backend/app/services/candle_fetcher.py` |
| eBest TR 호출 | `backend/app/services/ebest_service.py` |
| 합성 데이터(MOCK) | `backend/app/services/synthetic.py` |
| 보조지표 계산 | `backend/app/services/indicators.py` |
| 8패턴 + 신뢰도 + 보조필터 + 종합점수 | `backend/app/services/pattern_detector.py` |
| 전략 가중치/프리셋 | `backend/app/services/strategy.py` |
| 진입·청산 오케스트레이션 | `backend/app/services/trading_engine.py` |
| 리스크(사이징·손절·익절·한도) | `backend/app/services/risk.py` |
| 주문 실행(모의/실전) | `backend/app/services/broker.py` |
| 차트 API | `backend/app/routers/candles.py` |
| 시그널 스캔 API | `backend/app/routers/signals.py` |

---

*분석 기준 커밋: 현재 작업 트리. 수치(가중치·임계값·ATR 배수 등)는 코드 기본값이며
`.env`(TRADING_*) 및 UI에서 조정 가능.*
