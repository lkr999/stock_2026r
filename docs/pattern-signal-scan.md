# 대시보드 — 백테스트 기반 관심종목 선정 원리

대시보드(`/`)는 **가격 필터 → 후보 전체 백테스트 → OOS 순수익 기준 선정**의 3단계로 관심종목을 고른다.
데이터는 전부 eBest REST API에서만 가져오며 가짜/모의 데이터는 생성하지 않는다.

> 과거에는 대시보드가 캔들 패턴을 스캔해 신호를 보여줬지만, 지금은 **패턴 감지를 스캔에서 분리**하고
> 후보 종목을 실제 백테스트로 검증해 고른다. (단일 종목 패턴은 차트 화면의 `/api/patterns/{code}`에서 본다.)

---

## 1. 한눈에 보는 흐름

```
[① 가격 필터 스캔]        [② 후보 전체 백테스트]            [③ OOS 기준 선정]
시장/현재가/후보상한   ─▶  strategy-matrix                ─▶  종목별 베스트(통과) 전략
   │ ▶ 스캔                (모든 전략 × 모든 후보)              OOS 순수익 랭킹 → 상위 N
   ▼                       │                                  ▼
GET /api/universe          POST /api/backtest/strategy-matrix  watchlist + symbolStrategies
(현재가 필터만)            (per-(전략,종목) OOS 결과)          (종목별 전략 배정)
```

- **자동 실행 없음.** 각 단계는 버튼으로 수동 실행한다(eBest ~1콜/초 한도).
- 프론트 진입점: [`+page.svelte`](../frontend/src/routes/+page.svelte)의 `scan()` → `runBacktest()` → `selectFromMatrix()`.

---

## 2. 단계별 상세

### ① 가격 필터 스캔 — 후보 선별
- `GET /api/universe?market&min_price&max_price&limit` 호출 ([universe.rs](../backend/src/universe.rs)).
- `files/search_item.csv`(종목·코드·시장·**현재가**·전일종가·등락률·거래량)를 1회 로드해, **시장과 현재가 범위로만** 거른 뒤 상한(`limit`)까지 후보를 만든다. **패턴 감지·점수 계산은 하지 않는다.**
- 현재가는 CSV 스냅샷 값으로, 후보를 좁히는 1차 필터다.
- `후보 상한`은 ②단계 백테스트 부하(eBest 호출 수)를 좌우하므로 신중히. 기본 60종목.

### ② 후보 전체 백테스트 — 모든 전략 × 모든 종목
- `POST /api/backtest/strategy-matrix` `{ shcodes: 후보코드, tf, max_hold_bars }` ([backtest 라우터](../backend/src/routers/backtest.rs)).
- 모든 전략 프리셋(반전 4 + 단타 4)을 후보 종목마다 돌려, **인샘플 + walk-forward OOS** 결과를 낸다.
- `tf = "auto"`(기본)이면 각 전략을 **자신의 권장 타임프레임**(반전 5m · 단타 1m)으로 검증한다. 종목당 1m·5m를 모두 조회하므로 후보 수에 비례해 느리다.
- 응답의 `items`는 per-(전략, 종목) 행으로, 각 행에 `oos_avg_return`(거래당 OOS 순수익 %)·`tradeable`(통과 여부)·`tf`가 들어 있다.
- 대시보드는 이를 **종목(행) × 전략(열)** 격자로 피벗해, 각 셀에 OOS 순수익을 표시하고 종목별 베스트(통과) 전략을 굵게, 통과는 `✓`로 강조한다.

### ③ OOS 순수익 기준 선정 — 종목별 베스트 전략 배정
- `selectFromMatrix()`가 `items`에서 종목마다 **OOS 순수익이 가장 높은(그리고 `tradeable`) 전략**을 고른다.
- 그 OOS 순수익으로 종목을 랭킹해 `최소 OOS 순수익(%)`·`최대 선정 종목수`로 추려 관심종목으로 선정한다.
- 동시에 **종목별 전략을 `symbolStrategies` 스토어에 배정**한다(코드→전략). 이 배정은 자동매매에서 종목별로 다르게 적용된다.

> `tradeable(통과)` 판정: OOS 순수익 > 0, OOS 일관성 ≥ 60%, OOS 신호 ≥ 10 (모두 만족).
> `oos_avg_return`은 이미 **%단위**(예 0.15 = 0.15%)다.

---

## 3. 자동매매로의 연결 — 종목별 전략

선정 결과는 두 가지로 자동매매에 전달된다.

1. **관심종목**(`watchlist` 스토어) — 트레이딩 페이지에서 ★ 불러오기로 로드.
2. **종목별 전략**(`symbolStrategies` 스토어) — 트레이딩 시작 시 `symbol_strategies`(코드→전략)로 함께 전송.

엔진([engine.rs](../backend/src/engine.rs))은 종목마다 배정된 전략을, **그 전략의 권장 타임프레임**으로 거래한다(배정 없는 종목은 전역 전략·전역 TF). 리스크(손절·익절·사이징)는 전역 공통이고 **진입 전략과 타임프레임만 종목별로** 달라진다.

---

## 4. 성능·운영 메모

- **호출 비용**: ②단계가 무겁다. 종목당 약 1초/타임프레임 × 전략 검증. `auto`는 1m·5m 둘 다 조회한다. 후보 상한을 작게(예 30~60) 두는 것을 권장.
- **OOS 중심**: 표시·선정 모두 학습에 쓰지 않은 구간(out-of-sample) 기준이라 과최적화를 배제한다.
- **승률보다 기대값**: 전략은 손익비 2:1 구조라 승률이 40%대여도 OOS 순수익이 양수면 이기는 전략이다. (자세한 분석은 백테스트 검토 참고)

---

## 5. 관련 파일

| 파일 | 역할 |
|------|------|
| [`frontend/src/routes/+page.svelte`](../frontend/src/routes/+page.svelte) | 대시보드 3단계 UI(스캔·백테스트·선정)·격자 표 |
| [`frontend/src/lib/stores/symbolStrategies.ts`](../frontend/src/lib/stores/symbolStrategies.ts) | 종목별 전략 배정 스토어 |
| [`frontend/src/lib/api.ts`](../frontend/src/lib/api.ts) | `universe`·`strategyMatrix` 호출 |
| [`backend/src/universe.rs`](../backend/src/universe.rs) | CSV 유니버스 로드·가격 필터 |
| [`backend/src/routers/backtest.rs`](../backend/src/routers/backtest.rs) | strategy-matrix(전략×종목 OOS)·auto TF |
| [`backend/src/backtest.rs`](../backend/src/backtest.rs) | OOS 백테스트·walk-forward |
| [`backend/src/engine.rs`](../backend/src/engine.rs) | 종목별 전략·종목별 TF 자동매매 |
