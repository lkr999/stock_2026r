<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type UniverseItem, type EbestStatus, type EbestTestResult, type OosCapacity } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';
  import { symbolStrategies } from '$lib/stores/symbolStrategies';
  import { loadDashboardState, saveDashboardState } from '$lib/stores/dashboardState';
  import { tradingStatus } from '$lib/stores/tradingStatus';
  import WatchlistPicker from '$lib/components/WatchlistPicker.svelte';

  // 이전에 저장한 대시보드 상태(설정+결과)를 복원한다 (페이지 이동 후에도 유지).
  const dsaved = loadDashboardState();

  // eBest API 통신 상태/테스트
  let ebestStatus: EbestStatus | null = null;
  let ebestResult: EbestTestResult | null = null;
  let testCode = dsaved.testCode ?? '005930';
  let testing = false;
  let apiPanelOpen = dsaved.apiPanelOpen ?? true;

  async function loadEbestStatus() {
    try { ebestStatus = await api.ebestStatus(); } catch (e) { /* ignore */ }
  }
  async function runEbestTest() {
    testing = true; ebestResult = null;
    try { ebestResult = await api.ebestTest(testCode.trim() || '005930'); }
    catch (e) { error = String(e); }
    testing = false;
    loadEbestStatus();
  }

  let error = '';

  // ── 1단계: 가격 필터 스캔 (패턴 감지 없이, 현재가/시장 필터만으로 후보 선별) ──
  let market = dsaved.market ?? 'ALL';
  let minPrice: number | undefined = dsaved.minPrice ?? 1000;
  let maxPrice: number | undefined = dsaved.maxPrice ?? 100000;
  let candidateLimit = dsaved.candidateLimit ?? 60;          // 백테스트 대상 상한 (eBest 호출 한도 고려)
  let candidates: UniverseItem[] = dsaved.candidates ?? [];
  let scanning = false;
  let scanned = dsaved.scanned ?? false;
  // 후보 스캔이 쓰는 종목 스냅샷(files/search_item.csv)의 기준시각/경과일.
  // 이 CSV 는 자동 갱신되지 않으므로 오래되면 가격 필터가 무의미해진다.
  let universeAsOf = '';
  let universeAgeDays = -1;

  // ── 2단계: 후보 전체 백테스트 (모든 전략 × 모든 종목) ──
  // 전략 세대 — 'v2'(신형) | 'legacy'(기존) | 'all'(비교용). 자동매매에서 실제로
  // 돌릴 세대로 백테스트해야 선정 결과가 그대로 쓰인다. 'all' 은 두 세대를 나란히
  // 보기 위한 비교 모드이며, 이때는 선정 단계에서 세대를 따로 골라야 한다.
  let btGeneration: 'v2' | 'legacy' | 'all' = dsaved.btGeneration ?? 'v2';
  let btTf = dsaved.btTf ?? 'auto';                // 'auto' = 전략별 권장 TF
  let btHold = dsaved.btHold ?? 25;
  // 백테스트가 시뮬레이션할 청산 방식 — 자동매매 폼과 같은 의미로 보내야
  // "백테스트에선 됐는데 실거래는 안 되는" 괴리가 사라진다. 기존 전략은 이 값을
  // 그대로 쓰고, 신형(V2)은 전략이 자기 규칙으로 덮어쓴다.
  let btStopLossPct = dsaved.btStopLossPct ?? 0;      // 0 = ATR 배수 사용
  let btTakeProfitPct = dsaved.btTakeProfitPct ?? 0;  // 0 = ATR 배수 사용
  let btHardStopIntrabar = dsaved.btHardStopIntrabar ?? false;
  /** 심층 조회 봉수 — 연속조회로 과거를 더 끌어와 OOS 표본을 늘린다 (0 = 기본 예산). */
  let btHistoryBars = dsaved.btHistoryBars ?? 0;
  /** 익절 목표폭(%) — 종목별 실측 변동성으로 권장 보유봉수를 역산하는 데 쓴다. */
  let btTargetPct = dsaved.btTargetPct ?? 3;
  let matrix: any = dsaved.matrix ?? null;
  let backtesting = false;

  let capacity: OosCapacity | null = null;

  // ── 3단계: OOS 기준 관심종목 선정 + 종목별 베스트 전략 배정 ──
  // 선정 기준(모두 사용자 설정) — 각 종목의 OOS 순수익 1위(거래有) 전략을 기준으로 판정한다.
  // 기본값 = 기존 강건성(tradeable) 게이트 기준: 순수익>0 · 일관성≥60% · 신호≥10.
  // 입력란 placeholder/라벨 표기에도 같은 상수를 써서 단일 출처로 관리한다.
  const OOS_DEFAULTS = {
    minReturn: 0, minSignals: 10, minConsistency: 60, requireTradeable: false,
    rankBy: 'winlb', maxPick: 15,
    // ── 승률 중심 기준 (신규) ──
    minWinRate: 0,      // 최소 OOS 승률(%) — 실측 승률
    minWinEdge: 0,      // 최소 승률 여유(%p) = 실제승률 − 손익분기승률. 0 이상이어야 구조적으로 남는다
    minPayoff: 0,       // 최소 손익비 (평균이익 ÷ 평균손실)
    minRetention: 0,    // 최소 IS→OOS 수익 유지율(%) — 낮으면 과최적화
    maxMdd: 0,          // OOS 최악 폴드 낙폭 허용치(%, 0 = 제한없음)
  };
  let oosMinReturn = dsaved.oosMinReturn ?? OOS_DEFAULTS.minReturn;                  // 최소 OOS 순수익(%)
  let oosMinSignals = dsaved.oosMinSignals ?? OOS_DEFAULTS.minSignals;              // 최소 OOS 신호수

  let oosMinConsistency = dsaved.oosMinConsistency ?? OOS_DEFAULTS.minConsistency;  // 최소 OOS 일관성(%)
  let oosRequireTradeable = dsaved.oosRequireTradeable ?? OOS_DEFAULTS.requireTradeable; // 강건성 통과(✓)만
  let oosRankBy = dsaved.oosRankBy ?? OOS_DEFAULTS.rankBy;                          // 랭킹 기준: return|consistency|signals
  let oosMaxPick = dsaved.oosMaxPick ?? OOS_DEFAULTS.maxPick;                       // 최대 선정 종목 수
  // ── 승률 중심 선정 기준 ──
  let oosMinWinRate = dsaved.oosMinWinRate ?? OOS_DEFAULTS.minWinRate;
  let oosMinWinEdge = dsaved.oosMinWinEdge ?? OOS_DEFAULTS.minWinEdge;
  let oosMinPayoff = dsaved.oosMinPayoff ?? OOS_DEFAULTS.minPayoff;
  let oosMinRetention = dsaved.oosMinRetention ?? OOS_DEFAULTS.minRetention;
  let oosMaxMdd = dsaved.oosMaxMdd ?? OOS_DEFAULTS.maxMdd;
  let oosSelectMsg = dsaved.oosSelectMsg ?? '';

  // 선정에 쓸 전략 세대 — 격자에 두 세대가 다 있어도 배정은 한 세대로만 한다.
  // 세대를 섞으면 어떤 종목은 전략이 손절을 강제하고 어떤 종목은 폼 값을 쓰게 돼
  // 리스크 규칙이 종목마다 달라진다 (원인 추적이 불가능해짐).
  let pickGeneration: 'v2' | 'legacy' = dsaved.pickGeneration ?? 'v2';

  /**
   * 세대별 권장 선정 기준.
   *
   * V2 기준이 더 빡빡한 이유: v2 전략은 손익비 1.8 이상을 강제하므로 손익분기
   * 승률이 ~36%로 낮다. 그 구조에서도 승률 여유가 안 나오는 종목은 애초에 이
   * 전략과 맞지 않는 종목이다. 유지율(IS→OOS) 하한도 함께 걸어 과최적화된
   * 조합을 배제한다 — 실거래 130건이 무너진 이유가 정확히 이 두 가지였다.
   */
  const CRITERIA_PRESETS: Record<string, { label: string; hint: string; v: Record<string, number | boolean | string> }> = {
    v2: {
      label: '✨ V2 권장 기준',
      hint: '손익비 1.6↑ · 승률여유 5%p↑ · 유지율 40%↑ — v2 전략의 설계 전제를 실제로 만족하는 종목만',
      v: {
        oosMinReturn: 0.05, oosMinSignals: 10, oosMinConsistency: 60,
        oosMinWinRate: 0, oosMinWinEdge: 5, oosMinPayoff: 1.6,
        oosMinRetention: 40, oosMaxMdd: 0,
        oosRequireTradeable: true, oosRankBy: 'winlb', oosMaxPick: 12,
      },
    },
    legacy: {
      label: '기존 기준',
      hint: '순수익>0 · 신호 10↑ · 일관성 60%↑ (레거시 전략용 기본값)',
      v: {
        oosMinReturn: 0, oosMinSignals: 10, oosMinConsistency: 60,
        oosMinWinRate: 0, oosMinWinEdge: 0, oosMinPayoff: 0,
        oosMinRetention: 0, oosMaxMdd: 0,
        oosRequireTradeable: false, oosRankBy: 'winlb', oosMaxPick: 15,
      },
    },
  };
  const CRITERIA_KEYS = ['v2', 'legacy'] as const;
  function applyCriteriaPreset(key: 'v2' | 'legacy') {
    const v = CRITERIA_PRESETS[key].v as any;
    oosMinReturn = v.oosMinReturn; oosMinSignals = v.oosMinSignals;
    oosMinConsistency = v.oosMinConsistency; oosMinWinRate = v.oosMinWinRate;
    oosMinWinEdge = v.oosMinWinEdge; oosMinPayoff = v.oosMinPayoff;
    oosMinRetention = v.oosMinRetention; oosMaxMdd = v.oosMaxMdd;
    oosRequireTradeable = v.oosRequireTradeable; oosRankBy = v.oosRankBy;
    oosMaxPick = v.oosMaxPick;
    oosSelectMsg = '';
  }

  // 타임프레임별 walk-forward OOS 표본 용량 — (TF, 보유봉수) 조합이 최소 신호수를
  // 구조적으로 채울 수 있는지 실행 전에 알려준다. 백테스트는 포지션을 중첩하지
  // 않으므로 한 번 진입하면 최대 보유봉수만큼 봉을 소비한다:
  //   최대 OOS 신호 = 검정 총봉수 ÷ 보유봉수
  $: capRows = capacity?.timeframes ?? [];
  // auto 는 전략마다 TF가 달라지므로 실제 쓰이는 것 중 가장 빡빡한 쪽을 기준으로 본다.
  $: activeCaps = btTf === 'auto'
    ? capRows.filter((r) => r.tf === '1m' || r.tf === '5m')
    : capRows.filter((r) => r.tf === btTf);
  $: btSignalCap = activeCaps.length
    ? Math.min(...activeCaps.map((r) => Math.floor(r.oos_test_bars / Math.max(1, btHold))))
    : null;
  $: btHoldCeiling = activeCaps.length
    ? Math.min(...activeCaps.map((r) => r.max_hold_for_min_signals))
    : null;
  // ③단계에서 사용자가 정한 최소 신호수와 같은 기준으로 판정한다.
  $: btCapacityOk = btSignalCap == null || btSignalCap >= (oosMinSignals ?? 10);


  // 설정·결과가 바뀔 때마다 localStorage 에 저장 → 페이지 이동 후에도 복원된다.
  $: saveDashboardState({
    market, minPrice, maxPrice, candidateLimit, candidates, scanned,
    btGeneration, btTf, btHold, btHistoryBars, btTargetPct, matrix,
    btStopLossPct, btTakeProfitPct, btHardStopIntrabar,
    oosMinReturn, oosMinSignals, oosMinConsistency,
    oosRequireTradeable, oosRankBy, oosMaxPick, oosSelectMsg,
    oosMinWinRate, oosMinWinEdge, oosMinPayoff, oosMinRetention, oosMaxMdd,
    pickGeneration,
    testCode, apiPanelOpen,
  });

  // 1단계: 현재가/시장 필터로 후보 종목 목록만 가져온다 (패턴 스캔 아님).
  async function scan() {
    if (scanning) return;
    scanning = true; error = ''; matrix = null; oosSelectMsg = '';
    try {
      const r = await api.universe({ market, minPrice, maxPrice, limit: candidateLimit });
      candidates = r.items ?? [];
      universeAsOf = r.universe_as_of ?? '';
      universeAgeDays = r.universe_age_days ?? -1;
      scanned = true;
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }

  // 2단계: 가격 필터된 후보 전체를 선택한 세대의 전략으로 OOS 백테스트.
  async function runBacktest() {
    if (backtesting) return;
    const codes = candidates.map((c) => c.code);
    if (!codes.length) { error = '먼저 ▶ 스캔으로 가격 필터된 후보를 만드세요.'; return; }
    backtesting = true; error = ''; matrix = null; oosSelectMsg = '';
    try {
      matrix = await api.strategyMatrix({
        shcodes: codes, tf: btTf, max_hold_bars: btHold,
        history_bars: btHistoryBars, target_pct: btTargetPct,
        generation: btGeneration,
        risk: {
          // 고정%는 소수로 보낸다 (3 → 0.03). 0 이면 ATR 배수를 쓴다.
          stop_loss_pct: btStopLossPct > 0 ? btStopLossPct / 100 : 0,
          take_profit_pct: btTakeProfitPct > 0 ? btTakeProfitPct / 100 : 0,
          hard_stop_intrabar: btHardStopIntrabar,
        },
      });
      // 백테스트를 새로 돌리면 선정 세대를 방금 돌린 세대에 맞춘다.
      if (btGeneration !== 'all') pickGeneration = btGeneration;
    } catch (e) {
      error = String(e);
    } finally {
      backtesting = false;
    }
  }

  // 이 격자에 실제로 들어 있는 세대별 전략 이름.
  $: matrixGenerations = (matrix?.generations ?? { v2: [], legacy: [] }) as Record<string, string[]>;
  $: hasV2Cols = (matrixGenerations.v2 ?? []).length > 0;
  $: hasLegacyCols = (matrixGenerations.legacy ?? []).length > 0;
  $: isV2Strategy = (name: string) => (matrixGenerations.v2 ?? []).includes(name);

  /** 개별 전략(셀)이 설정한 선정 기준을 통과하는지. (비율 필드는 0~1 이라 %로 환산해 비교) */
  function passesCriteria(it: any): boolean {
    return (
      it.ok &&
      // 선정 세대 밖의 전략은 후보에서 제외 — 배정이 두 세대에 걸치지 않게 한다.
      (it.generation ?? 'legacy') === pickGeneration &&
      (it.oos_total_signals ?? 0) > 0 &&
      (it.oos_avg_return ?? 0) >= oosMinReturn &&
      (it.oos_total_signals ?? 0) >= oosMinSignals &&
      (it.oos_consistency ?? 0) * 100 >= oosMinConsistency &&
      // ── 승률 중심 기준 ──
      (it.oos_win_rate ?? 0) * 100 >= oosMinWinRate &&
      (it.oos_win_edge ?? 0) * 100 >= oosMinWinEdge &&
      (oosMinPayoff <= 0 || (it.oos_payoff ?? 0) >= oosMinPayoff) &&
      // 유지율은 IS 가 흑자일 때만 산출되므로 null 이면 통과시킨다.
      (oosMinRetention <= 0 || it.is_oos_retention == null || it.is_oos_retention * 100 >= oosMinRetention) &&
      (oosMaxMdd <= 0 || Math.abs(it.oos_worst_mdd ?? 0) <= oosMaxMdd) &&
      (!oosRequireTradeable || !!it.tradeable)
    );
  }

  // 랭킹 기준별 정렬 키. 기본값 winlb = 승률의 Wilson 하한 (소표본 착시 방지).
  function rankVal(v: { oos: number; consistency: number; signals: number; winRate: number; winLb: number; winEdge: number; payoff: number }): number {
    switch (oosRankBy) {
      case 'consistency': return v.consistency;
      case 'signals': return v.signals;
      case 'winrate': return v.winRate;
      case 'winlb': return v.winLb;
      case 'winedge': return v.winEdge;
      case 'payoff': return v.payoff;
      default: return v.oos;
    }
  }

  // 3단계: 백테스트 결과에서 종목마다 랭킹 기준 1위(거래有) 전략을 고른 뒤,
  // 사용자가 설정한 선정 기준(최소 순수익·신호수·일관성·강건성 통과)으로 걸러
  // 랭킹 기준으로 정렬해 최대 N종목을 관심종목으로 선정하고 전략을 배정한다.
  function selectFromMatrix() {
    if (!matrix?.items?.length) { error = '먼저 ▶ 백테스트를 실행하세요.'; return; }
    type Pick = {
      strategy: string; tf: string; oos: number; consistency: number; signals: number;
      tradeable: boolean; winRate: number; winLb: number; winEdge: number; payoff: number;
    };
    const passes = passesCriteria;
    // 종목마다 "기준을 통과하는 전략 중 OOS 순수익 1위"를 대표로 고른다.
    // → 순수익 1위 전략이 기준 미달이어도, 같은 종목에 통과하는 강건한 전략이
    //   있으면 그 전략으로 종목을 살린다(먼저 고르고 거르지 않는다).
    const bySymbol = new Map<string, Pick>();
    for (const it of matrix.items) {
      if (!passes(it)) continue;
      const cur = bySymbol.get(it.shcode);
      const cand: Pick = {
        strategy: it.strategy, tf: it.tf,
        oos: it.oos_avg_return ?? 0,
        consistency: it.oos_consistency ?? 0,   // 0~1 비율
        signals: it.oos_total_signals ?? 0,
        tradeable: !!it.tradeable,
        winRate: it.oos_win_rate ?? 0,
        winLb: it.oos_win_rate_lb ?? 0,
        winEdge: it.oos_win_edge ?? 0,
        payoff: it.oos_payoff ?? 0,
      };
      // 종목 대표 전략은 '현재 랭킹 기준'으로 고른다 — 승률로 랭킹하면서
      // 대표는 순수익 1위를 뽑으면 선정 결과와 랭킹이 어긋난다.
      if (!cur || rankVal(cand) > rankVal(cur)) bySymbol.set(it.shcode, cand);
    }
    // 랭킹 기준으로 종목 정렬 후 상위 N.
    const picks = [...bySymbol.entries()].sort((a, b) => rankVal(b[1]) - rankVal(a[1])).slice(0, oosMaxPick);
    if (!picks.length) {
      const genLabel = pickGeneration === 'v2' ? '신형(V2)' : '기존(레거시)';
      const noneOfGen = !matrix.items.some((it: any) => (it.generation ?? 'legacy') === pickGeneration);
      oosSelectMsg = noneOfGen
        ? `이번 백테스트에 ${genLabel} 전략이 없습니다 — ②단계에서 세대를 ${genLabel}(또는 전체 비교)로 두고 다시 실행하세요.`
        : `${genLabel} 전략 중 설정한 기준(${criteriaSummary})을 통과한 종목이 없습니다. 기준을 완화하거나 후보를 늘려보세요.`;
      return;
    }
    watchlist.clear();
    // 전략 이름과 함께 백테스트에 실제 쓰인 TF(v.tf) 도 배정에 저장 → 자동매매가
    // 백테스트와 동일한 타임프레임으로 이 전략을 돌린다.
    const map: Record<string, { strategy: string; tf: string }> = {};
    for (const [code, v] of picks) { watchlist.add(code); map[code] = { strategy: v.strategy, tf: v.tf }; }
    symbolStrategies.replace(map);
    const preview = picks.slice(0, 6)
      .map(([c, v]) => `${c}→${v.strategy}·${v.tf}(승률 ${(v.winRate * 100).toFixed(0)}% · ${v.oos.toFixed(2)}%)`)
      .join(', ');
    const genLabel = pickGeneration === 'v2' ? '신형(V2)' : '기존(레거시)';
    oosSelectMsg = `${genLabel} ${picks.length}종목 선정 [${criteriaSummary}] — ${preview}${picks.length > 6 ? ' …' : ''}`;
  }

  // 현재 선정 기준 요약 문구 (메시지/설명에 사용).
  const rankLabel: Record<string, string> = {
    return: 'OOS 순수익', consistency: '일관성', signals: '신호수',
    winrate: '승률', winlb: '승률하한', winedge: '승률여유', payoff: '손익비',
  };
  $: criteriaSummary = [
    `순수익≥${oosMinReturn}%`,
    oosMinSignals > 0 ? `신호≥${oosMinSignals}` : null,
    oosMinConsistency > 0 ? `일관성≥${oosMinConsistency}%` : null,
    oosMinWinRate > 0 ? `승률≥${oosMinWinRate}%` : null,
    oosMinWinEdge > 0 ? `승률여유≥${oosMinWinEdge}%p` : null,
    oosMinPayoff > 0 ? `손익비≥${oosMinPayoff}` : null,
    oosMinRetention > 0 ? `유지율≥${oosMinRetention}%` : null,
    oosMaxMdd > 0 ? `낙폭≤${oosMaxMdd}%` : null,
    oosRequireTradeable ? '강건성✓' : null,
    `${rankLabel[oosRankBy] ?? 'OOS 순수익'}순 상위${oosMaxPick}`,
  ].filter(Boolean).join(' · ');

  /** 격자 셀에 표시할 값: 순수익 | 승률 | 승률하한 | 손익비. */
  let gridMetric = 'winrate';
  const GRID_METRICS: Record<string, { label: string; get: (it: any) => number | null; fmt: (v: number) => string }> = {
    return:  { label: 'OOS 순수익(%)', get: (it) => it.oos_avg_return ?? null, fmt: (v) => v.toFixed(2) },
    winrate: { label: 'OOS 승률(%)',   get: (it) => (it.oos_win_rate ?? null) === null ? null : it.oos_win_rate * 100, fmt: (v) => v.toFixed(0) },
    winlb:   { label: '승률하한(%)',    get: (it) => (it.oos_win_rate_lb ?? null) === null ? null : it.oos_win_rate_lb * 100, fmt: (v) => v.toFixed(0) },
    payoff:  { label: '손익비',         get: (it) => it.oos_payoff ?? null, fmt: (v) => v.toFixed(2) },
  };
  // 셀 색상 기준선 — 순수익은 0, 승률류는 손익분기 승률, 손익비는 1.
  function cellPositive(it: any, v: number): boolean {
    if (gridMetric === 'return') return v >= 0;
    if (gridMetric === 'payoff') return v >= 1;
    return v >= (it.oos_breakeven_win_rate ?? 0.5) * 100;
  }
  function cellTitle(it: any): string {
    const p = (x: number | undefined | null, d = 0) => x == null ? '-' : (x * 100).toFixed(d) + '%';
    return [
      `${it.strategy} · ${it.tf}`,
      `OOS 순수익 ${(it.oos_avg_return ?? 0).toFixed(2)}% · 신호 ${it.oos_total_signals ?? 0}건`,
      `승률 ${p(it.oos_win_rate)} (하한 ${p(it.oos_win_rate_lb)}) / 손익분기 ${p(it.oos_breakeven_win_rate)}`,
      `손익비 ${(it.oos_payoff ?? 0).toFixed(2)} · 평균이익 ${(it.oos_avg_win ?? 0).toFixed(2)}% / 평균손실 ${(it.oos_avg_loss ?? 0).toFixed(2)}%`,
      `최악 낙폭 ${(it.oos_worst_mdd ?? 0).toFixed(2)}%`,
      it.is_oos_retention == null ? 'IS→OOS 유지율 -' : `IS→OOS 유지율 ${p(it.is_oos_retention)}`,
      it.bar_sigma_pct ? `봉 변동성 σ ${it.bar_sigma_pct.toFixed(3)}% → 목표 ${matrix?.target_pct ?? 0}% 도달 권장 보유 ${it.recommended_hold_bars}봉` : '',
    ].filter(Boolean).join('\n');
  }
  /** 선정 기준으로 실제 통과한 종목 수 미리보기 (버튼 누르기 전에 확인). */
  $: passPreview = (() => {
    if (!matrix?.items?.length) return null;
    const codes = new Set<string>();
    for (const it of matrix.items) if (passesCriteria(it)) codes.add(it.shcode);
    return codes.size;
  })();

  // 백테스트 결과를 종목(행) × 전략(열) 격자로 변환 — 각 셀은 OOS 순수익(%).
  //
  // `best` 는 격자 전체의 1위(참고용), `bestOfPick` 은 **선정 세대 안에서의** 1위다.
  // ★ 버튼은 후자를 배정한다 — 신형으로 돌리려는데 별 하나로 레거시 전략이
  // 배정되면 그 종목만 다른 리스크 규칙으로 매매된다.
  type Row = {
    code: string; name: string; cells: Record<string, any>;
    best: string | null; bestOos: number;
    bestOfPick: string | null; bestOfPickOos: number;
  };
  function buildRows(m: any, gen: string): Row[] {
    if (!m?.items?.length) return [];
    const bySym = new Map<string, Row>();
    for (const it of m.items) {
      let row = bySym.get(it.shcode);
      if (!row) {
        row = { code: it.shcode, name: it.name, cells: {}, best: null, bestOos: -Infinity, bestOfPick: null, bestOfPickOos: -Infinity };
        bySym.set(it.shcode, row);
      }
      row.cells[it.strategy] = it;
      // 베스트 = 실제 거래(OOS 신호>0)가 있은 전략 중 OOS 순수익 1위.
      const traded = it.ok && (it.oos_total_signals ?? 0) > 0;
      if (traded && it.oos_avg_return > row.bestOos) {
        row.bestOos = it.oos_avg_return;
        row.best = it.strategy;
      }
      if (traded && (it.generation ?? 'legacy') === gen && it.oos_avg_return > row.bestOfPickOos) {
        row.bestOfPickOos = it.oos_avg_return;
        row.bestOfPick = it.strategy;
      }
    }
    return [...bySym.values()].sort((a, b) => (b.bestOos === -Infinity ? -1e9 : b.bestOos) - (a.bestOos === -Infinity ? -1e9 : a.bestOos));
  }

  $: strategyCols = (matrix?.strategies ?? []) as string[];
  $: tfByStrategy = Object.fromEntries(((matrix?.by_strategy ?? []) as any[]).map((s) => [s.strategy, s.tf]));
  $: perSymbolRows = buildRows(matrix, pickGeneration);

  // 세대별 집계 — '전체 비교' 모드에서 어느 세대가 실제로 나았는지 한 줄로 본다.
  $: genSummary = (() => {
    const rows = (matrix?.by_strategy ?? []) as any[];
    if (!rows.length) return [];
    return (['v2', 'legacy'] as const)
      .map((g) => {
        const rs = rows.filter((r) => (r.generation ?? 'legacy') === g && r.graded_count > 0);
        if (!rs.length) return null;
        const sig = rs.reduce((s, r) => s + (r.oos_total_signals ?? 0), 0);
        // 신호수 가중 평균 — 표본이 큰 전략이 더 많이 반영되게 한다.
        const wret = sig ? rs.reduce((s, r) => s + (r.oos_avg_return ?? 0) * (r.oos_total_signals ?? 0), 0) / sig : 0;
        const best = rs.reduce((a, b) => ((b.oos_avg_return ?? 0) > (a.oos_avg_return ?? 0) ? b : a));
        return {
          gen: g,
          label: g === 'v2' ? '신형 V2' : '기존 레거시',
          strategies: rs.length,
          tradeable: rs.reduce((s, r) => s + (r.tradeable_count ?? 0), 0),
          signals: sig,
          avgReturn: wret,
          best: best.strategy,
          bestReturn: best.oos_avg_return ?? 0,
        };
      })
      .filter(Boolean) as any[];
  })();

  // ── OOS 선정이 실제 자동매매에 반영됐는지 확인 ──
  // 선정 자체는 watchlist/symbolStrategies 스토어에 즉시 저장되지만, 그 값은
  // 자동매매 페이지에서 "불러오기 → 시작"을 눌러야 엔진에 반영된다. 이 차이를
  // 눈으로 바로 확인할 수 있도록 실행 중인 엔진의 워치리스트와 비교해 보여준다.
  $: engineWatchlist = new Set($tradingStatus.watchlist ?? []);
  $: notReflectedCodes = $watchlist.filter((c) => !engineWatchlist.has(c));

  onMount(loadEbestStatus);
  // OOS 표본 용량표 (실패해도 화면 동작에는 영향 없음 — 배너만 숨겨진다).
  onMount(async () => {
    try { capacity = await api.backtestCapacity(); } catch { capacity = null; }
  });
</script>

<header>
  <h1>백테스트 기반 관심종목 선정 대시보드</h1>
  <p class="sub">
    ① <b>가격 필터</b>로 후보 선별 → ② <b>전략 세대를 골라</b> 후보 전체 백테스트 →
    ③ 그 세대 안에서 관심종목 선정 + 종목별 베스트 전략 자동 배정
  </p>
  <p class="sub gen-lead">
    ✨ <b>신형 V2 전략을 쓰려면</b> ②에서 <b>신형 V2 전용</b>을 고르세요 — 손절·익절이 전략 규칙으로
    시뮬레이션되고, ③에서 <b>V2 권장 기준</b>을 적용하면 그 설계 전제(손익비 1.8↑)를 실제로
    만족하는 종목만 선정됩니다. 세대를 섞어 배정하지 않으므로 자동매매에서도 규칙이 일관됩니다.
  </p>
</header>

<!-- eBest API 통신 상태/테스트 -->
<div class="api-panel">
  <div class="api-head">
    <div class="api-title">
      <button class="toggle" on:click={() => (apiPanelOpen = !apiPanelOpen)}>{apiPanelOpen ? '▾' : '▸'}</button>
      <b>eBest API 통신 상태</b>
      {#if ebestStatus}
        <span class="badge {ebestStatus.token_ok ? 'live' : 'off'}">
          {ebestStatus.token_ok ? 'eBest 연결됨' : 'eBest 미연결'}
        </span>
        <span class="badge {ebestStatus.has_keys ? 'ok' : 'off'}">키 {ebestStatus.has_keys ? '설정됨' : '없음'}</span>
        <span class="badge {ebestStatus.token_ok ? 'ok' : 'off'}">토큰 {ebestStatus.token_ok ? '정상' : '없음'}</span>
      {:else}
        <span class="badge off">상태 확인중…</span>
      {/if}
    </div>
    <div class="api-actions">
      <input class="code-in" bind:value={testCode} placeholder="종목코드" />
      <button class="test-btn" on:click={runEbestTest} disabled={testing}>
        {testing ? '테스트중…' : '▶ 통신 테스트'}
      </button>
    </div>
  </div>

  {#if apiPanelOpen && ebestResult}
    <div class="api-result">
      <div class="rsum {ebestResult.ok ? 'ok' : 'fail'}">
        {ebestResult.ok ? '✓ 전체 통신 정상' : '✗ 일부 통신 실패'}
        · {ebestResult.name || ebestResult.code} ({ebestResult.code})
        · 출처 eBest 실시간
      </div>
      <table>
        <thead><tr><th>점검 항목</th><th>TR</th><th>결과</th><th>응답(ms)</th><th>상세</th></tr></thead>
        <tbody>
          {#each ebestResult.checks as ck}
            <tr>
              <td>{ck.name}</td>
              <td class="tr">{ck.tr}</td>
              <td class={ck.ok ? 'ok' : 'fail'}>{ck.ok ? '✓ 성공' : '✗ 실패'}</td>
              <td class="num">{ck.latency_ms}</td>
              <td class="detail">{ck.detail}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
  {#if apiPanelOpen && ebestStatus && !ebestStatus.token_ok}
    <p class="api-hint">⚠️ eBest 연결 안 됨 — 모든 데이터는 eBest API 에서만 가져옵니다. <code>.env</code>에 <code>EBEST_APP_KEY</code>/<code>EBEST_APP_SECRET</code>를 설정하세요. (가짜/모의 데이터는 생성하지 않습니다)</p>
  {/if}
</div>

<div class="picker-wrap">
  <WatchlistPicker {market} {minPrice} {maxPrice} />
</div>

<!-- ① 가격 필터 스캔 -->
<div class="step">
  <div class="step-head"><span class="step-no">①</span> 가격 필터로 후보 선별 <span class="step-sub">— 패턴 감지 없이 현재가/시장 필터만 적용</span></div>
  <div class="controls">
    <div class="group">
      <label>시장</label>
      <select bind:value={market}><option>ALL</option><option>KOSPI</option><option>KOSDAQ</option></select>
    </div>
    <div class="group"><label>현재가 최소</label><input type="number" bind:value={minPrice} step="500" min="0" /></div>
    <div class="group"><label>현재가 최대</label><input type="number" bind:value={maxPrice} step="500" min="0" /></div>
    <div class="group"><label>후보 상한(종목수)</label><input type="number" bind:value={candidateLimit} step="10" min="1" /></div>
    <button class="refresh" on:click={scan} disabled={scanning}>
      {scanning ? '스캔중…' : scanned ? '↻ 다시 스캔' : '▶ 스캔'}
    </button>
    {#if scanned}<span class="cand-count">후보 <b>{candidates.length}</b>종목</span>{/if}
  </div>
  {#if scanned && universeAgeDays >= 0}
    <div class="universe-age {universeAgeDays > 7 ? 'stale' : 'fresh'}">
      {universeAgeDays > 7 ? '⚠️' : '✅'}
      종목 스냅샷 기준 <b>{universeAsOf.slice(0, 10)}</b> ({universeAgeDays}일 경과)
      {#if universeAgeDays > 7}
        — <b>가격 필터가 {universeAgeDays}일 전 현재가로 걸러집니다.</b>
        지금 가격대와 다른 종목이 후보에 섞이므로 <code>files/search_item.csv</code> 를 갱신하세요.
        (백테스트·자동매매의 캔들 데이터는 eBest 실시간 조회라 영향 없습니다 — 후보 선별 단계만 해당)
      {/if}
    </div>
  {/if}
  <div style="display:none">
    <a class="to-trading" href="/trading">관심종목으로 자동매매 →</a>
  </div>
</div>

<!-- ② 후보 전체 백테스트 -->
<div class="step">
  <div class="step-head"><span class="step-no">②</span> 후보 전체 백테스트 <span class="step-sub">— 종목 × 전략 OOS 검증</span></div>

  <!-- 전략 세대 선택 — 자동매매에서 돌릴 세대로 검증해야 선정이 그대로 쓰인다. -->
  <div class="controls gen-row">
    <span class="grp-title">전략 세대</span>
    <div class="gen-seg">
      <button class:on={btGeneration==='v2'} class:v2={btGeneration==='v2'} on:click={() => (btGeneration = 'v2')}>
        ✨ 신형 V2 전용
      </button>
      <button class:on={btGeneration==='legacy'} on:click={() => (btGeneration = 'legacy')}>
        기존 레거시 전용
      </button>
      <button class:on={btGeneration==='all'} on:click={() => (btGeneration = 'all')}>
        전체 비교 (15종)
      </button>
    </div>
    <span class="gen-hint">
      {#if btGeneration === 'v2'}
        v2 전략 5종만 검증합니다. 손절·익절은 <b>각 전략의 규칙</b>으로 시뮬레이션되므로 아래 청산 설정은 무시됩니다.
      {:else if btGeneration === 'legacy'}
        기존 전략 10종만 검증합니다. 손절·익절은 <b>아래 청산 설정 그대로</b> 시뮬레이션됩니다.
      {:else}
        두 세대를 나란히 비교합니다 — 시간이 약 1.5배 걸리고, ③단계에서 선정 세대를 따로 골라야 합니다.
      {/if}
    </span>
  </div>

  <!-- 청산 방식 — 레거시 전략의 백테스트를 실거래와 일치시키기 위한 입력. -->
  <div class="controls sub-controls" class:muted-row={btGeneration === 'v2'}>
    <span class="grp-title">청산 시뮬레이션{#if btGeneration === 'v2'} <em>(V2는 전략이 강제)</em>{/if}</span>
    <div class="group">
      <label title="자동매매 폼의 '손절 비율'과 같은 값. 0 이면 ATR 배수를 씁니다.">손절 고정(%)</label>
      <input class="narrow" type="number" bind:value={btStopLossPct} min="0" step="0.5" placeholder="0 = ATR" />
    </div>
    <div class="group">
      <label title="자동매매 폼의 '익절 비율'과 같은 값. 0 이면 ATR 배수를 씁니다.">익절 고정(%)</label>
      <input class="narrow" type="number" bind:value={btTakeProfitPct} min="0" step="0.5" placeholder="0 = ATR" />
    </div>
    <label class="chk" title="OFF면 손절을 '닫힌 봉의 종가'로만 판정합니다 — 실거래에서 손절이 설정값보다 훨씬 깊게 체결되는 현상을 그대로 재현합니다.">
      <input type="checkbox" bind:checked={btHardStopIntrabar} /> 실시간 손절(intrabar)
    </label>
    {#if btGeneration !== 'v2' && btStopLossPct > 0 && !btHardStopIntrabar}
      <span class="overshoot-warn">
        ⚠️ 고정 손절 + 실시간 손절 OFF = <b>봉 종가로만 손절</b> — 실제 손실이 설정값보다 깊어집니다.
        지금 설정은 실거래와 같은 조건이므로, 백테스트가 나쁘게 나오는 것이 정상입니다.
      </span>
    {/if}
  </div>

  <div class="controls">
    <div class="group">
      <label>타임프레임</label>
      <select bind:value={btTf}>
        <option value="auto">auto (전략별 권장 TF)</option>
        {#each ['1m','3m','5m','10m','15m','30m','60m','1d'] as t}<option value={t}>{t}</option>{/each}
      </select>
    </div>
    <div class="group"><label title="매수 후 최대 보유 봉수 (도달 시 시간청산). 1 이상 자유 입력.">보유봉수</label>
      <div class="hold-input">
        <input type="number" bind:value={btHold} min="1" max="2000" step="1"
          on:change={() => { if (!btHold || btHold < 1) btHold = 1; }} />
        <div class="presets">
          {#each [5,10,25,40,60,100,200] as h}
            <button type="button" class:on={btHold === h} on:click={() => btHold = h}>{h}</button>
          {/each}
        </div>
      </div>
    </div>
    <div class="group">
      <label title="연속조회로 과거 봉을 더 끌어와 OOS 표본을 늘립니다. 0 = 타임프레임 기본 예산(1m·3m 500 / 5m 300 / 10m·15m 200 / 30m·60m 100).">
        심층 조회봉수
      </label>
      <div class="hold-input">
        <input type="number" bind:value={btHistoryBars} min="0" max="2000" step="100" />
        <div class="presets">
          {#each [0, 500, 1000, 1500, 2000] as h}
            <button type="button" class:on={btHistoryBars === h} on:click={() => btHistoryBars = h}>{h === 0 ? '기본' : h}</button>
          {/each}
        </div>
      </div>
    </div>
    <div class="group">
      <label title="익절 목표폭. 종목별 실측 변동성(σ)으로 '이 목표에 닿으려면 몇 봉이 필요한지'를 역산해 표에 표시합니다.">
        목표폭(%)
      </label>
      <input class="narrow" type="number" bind:value={btTargetPct} min="0" step="0.5" />
    </div>
    <button class="bt-btn" class:v2={btGeneration === 'v2'} on:click={runBacktest} disabled={backtesting || !candidates.length}>
      {backtesting
        ? '백테스트 중…'
        : `▶ 후보 ${candidates.length}종목 × ${btGeneration === 'v2' ? 'V2 5종' : btGeneration === 'legacy' ? '레거시 10종' : '전체 15종'} 백테스트`}
    </button>
    <span class="bt-note">
      auto는 종목당 1m·5m를 모두 조회하므로 후보 수에 비례해 시간이 걸립니다(종목당 약 1초/봉).
      {#if btHistoryBars > 0}<b class="warn-inline">심층 조회는 종목·TF당 최대 4회 호출 → 소요시간이 약 4배</b>{/if}
    </span>
  </div>
  {#if btSignalCap != null}
    <div class="capacity {btCapacityOk ? 'ok' : 'bad'}">
      <div class="cap-head">
        {btCapacityOk ? '✅' : '⚠️'}
        <b>{btTf === 'auto' ? 'auto(1m·5m)' : btTf}</b> · 보유 <b>{btHold}</b>봉 →
        종목당 OOS 신호 <b>최대 {btSignalCap}건</b> (③단계 최소 기준 {oosMinSignals ?? 10}건)
      </div>
      <div class="cap-body">
        {#each activeCaps as r}
          <span class="cap-chip">
            <b>{r.tf}</b> 조회 {r.candles}봉 → 검정 {r.oos_test_bars}봉({r.folds}폴드 × {r.fold_bars}봉)
            · 보유봉수 상한 <b>{r.max_hold_for_min_signals}</b>
          </span>
        {/each}
      </div>
      {#if !btCapacityOk}
        <div class="cap-warn">
          백테스트는 포지션을 중첩하지 않아 <b>한 번 진입하면 최대 보유봉수만큼 봉을 소비</b>합니다.
          지금 조합은 <b>전략 성능과 무관하게</b> 최소 신호수를 채울 수 없어 선정 결과가 비게 됩니다 —
          {#if btHoldCeiling}보유봉수를 <b>{btHoldCeiling}봉 이하</b>(여유 있게는 {Math.max(1, Math.floor(btHoldCeiling / 2))}봉 부근)로 줄이거나{/if}
          더 짧은 타임프레임을 선택하세요.
        </div>
      {/if}
    </div>
  {/if}

  {#if matrix && hasV2Cols}
    <div class="mkt-caveat">
      ℹ️ <b>V2 전략의 시장 필터는 백테스트에 반영되지 않습니다.</b>
      (프록시 ETF 캔들 정렬이 필요해 시뮬레이션에서 제외) — 실거래는 시장이 리스크오프이거나
      종목이 시장 대비 약할 때 진입을 <b>추가로</b> 차단하므로, 실제 진입 건수는 아래 숫자보다
      <b>적습니다</b>. 즉 아래 신호수는 상한이고, 승률·손익비는 보수적으로 읽으면 됩니다.
    </div>
  {/if}
</div>

<!-- ③ OOS 기준 선정 (선정 기준 사용자 설정) -->
<div class="step">
  <div class="step-head"><span class="step-no">③</span> 관심종목 선정 기준 설정 <span class="step-sub">— 종목별 베스트 전략 자동 배정</span></div>

  <!-- 선정 세대 + 권장 기준 프리셋 -->
  <div class="controls gen-row">
    <span class="grp-title">선정 세대</span>
    <div class="gen-seg">
      <button class:on={pickGeneration==='v2'} class:v2={pickGeneration==='v2'}
        disabled={!!matrix && !hasV2Cols}
        on:click={() => (pickGeneration = 'v2')}>✨ 신형 V2</button>
      <button class:on={pickGeneration==='legacy'}
        disabled={!!matrix && !hasLegacyCols}
        on:click={() => (pickGeneration = 'legacy')}>기존 레거시</button>
    </div>
    <div class="preset-btns">
      {#each CRITERIA_KEYS as k}
        <button class="crit-preset" class:v2={k === 'v2'} title={CRITERIA_PRESETS[k].hint}
          on:click={() => applyCriteriaPreset(k)}>
          {CRITERIA_PRESETS[k].label} 적용
        </button>
      {/each}
    </div>
    <span class="gen-hint">
      선정·배정은 <b>한 세대 안에서만</b> 이뤄집니다 — 세대를 섞으면 종목마다 손절 규칙이
      달라져(전략 강제 vs 폼 값) 결과를 해석할 수 없게 됩니다.
    </span>
  </div>

  <div class="controls">
    <div class="group"><label>최소 OOS 순수익(%) <span class="dflt">기본 {OOS_DEFAULTS.minReturn}</span></label><input type="number" bind:value={oosMinReturn} step="0.05" placeholder={String(OOS_DEFAULTS.minReturn)} /></div>
    <div class="group"><label>최소 OOS 신호수 <span class="dflt">기본 {OOS_DEFAULTS.minSignals}</span></label><input type="number" bind:value={oosMinSignals} step="1" min="0" placeholder={String(OOS_DEFAULTS.minSignals)} /></div>
    <div class="group"><label>최소 일관성(%) <span class="dflt">기본 {OOS_DEFAULTS.minConsistency}</span></label><input type="number" bind:value={oosMinConsistency} step="5" min="0" max="100" placeholder={String(OOS_DEFAULTS.minConsistency)} /></div>
    <div class="group"><label>최대 선정 종목수 <span class="dflt">기본 {OOS_DEFAULTS.maxPick}</span></label><input type="number" bind:value={oosMaxPick} step="1" min="1" placeholder={String(OOS_DEFAULTS.maxPick)} /></div>
    <div class="group">
      <label>랭킹 기준 <span class="dflt">기본 승률하한</span></label>
      <select bind:value={oosRankBy}>
        <option value="winlb">승률하한 (Wilson · 소표본 착시 방지)</option>
        <option value="winrate">승률 (실측)</option>
        <option value="winedge">승률 여유 (승률 − 손익분기)</option>
        <option value="payoff">손익비</option>
        <option value="return">OOS 순수익</option>
        <option value="consistency">일관성</option>
        <option value="signals">신호수</option>
      </select>
    </div>
    <label class="chk" title="OOS 순수익>0 · 일관성≥60% · 신호≥10 을 모두 통과(✓)한 종목만 선정">
      <input type="checkbox" bind:checked={oosRequireTradeable} /> 강건성 통과(✓)만 <span class="dflt">기본 해제</span>
    </label>
  </div>

  <!-- 승률 중심 기준 (신규) -->
  <div class="controls sub-controls">
    <span class="grp-title">승률 기준</span>
    <div class="group">
      <label title="OOS 전체 거래를 한 풀로 모은 실측 승률">최소 승률(%) <span class="dflt">0=미적용</span></label>
      <input type="number" bind:value={oosMinWinRate} step="5" min="0" max="100" />
    </div>
    <div class="group">
      <label title="실제 승률 − 손익분기 승률. 손익분기 승률 = 1÷(1+손익비). 이 값이 0보다 커야 구조적으로 수익이 남습니다.">
        최소 승률여유(%p) <span class="dflt">0=미적용</span>
      </label>
      <input type="number" bind:value={oosMinWinEdge} step="1" min="-50" max="50" />
    </div>
    <div class="group">
      <label title="평균이익 ÷ 평균손실. 1.5 면 이길 때 1.5배 더 번다는 뜻.">최소 손익비 <span class="dflt">0=미적용</span></label>
      <input type="number" bind:value={oosMinPayoff} step="0.1" min="0" />
    </div>
    <div class="group">
      <label title="OOS 순수익 ÷ 인샘플 순수익. 100% 면 학습구간 성능이 그대로 유지된 것, 낮으면 과최적화 신호.">
        최소 유지율(%) <span class="dflt">0=미적용</span>
      </label>
      <input type="number" bind:value={oosMinRetention} step="10" min="0" />
    </div>
    <div class="group">
      <label title="OOS 폴드 중 가장 나빴던 누적 낙폭. 이 값을 넘으면 제외.">최대 낙폭(%) <span class="dflt">0=미적용</span></label>
      <input type="number" bind:value={oosMaxMdd} step="1" min="0" />
    </div>
    <button class="bt-btn pick" on:click={selectFromMatrix} disabled={!matrix}>★ 기준으로 선정</button>
  </div>

  <div class="bt-note">
    현재 기준: <b>{criteriaSummary}</b> — 종목마다 <b>랭킹 기준 1위</b> 전략을 골라 위 조건으로 걸러 관심종목으로 선정하고, 자동매매에 종목별로 다르게 적용합니다.
  </div>
  <details class="crit-help">
    <summary>승률을 올리려면 어떤 기준을 쓰나요?</summary>
    <ul>
      <li><b>승률하한(기본)</b>: 표본이 적을수록 승률을 깎아 평가합니다. <code>3전 3승(100%)</code> 은 <code>100전 70승(70%)</code> 보다 아래로 내려갑니다 — 우연히 몇 번 맞은 종목이 상위에 올라오는 것을 막는 가장 효과적인 기준입니다.</li>
      <li><b>승률 여유</b>: 승률만 높고 손익비가 나쁘면 결국 잃습니다. <code>승률 − 1÷(1+손익비)</code> 가 <b>양수</b>인 종목만 남기세요. 이 한 줄이 “승률은 높은데 계좌는 준다”를 막습니다.</li>
      <li><b>유지율</b>: 인샘플만 좋고 OOS 에서 무너지는 과최적화 종목을 걸러냅니다. <b>50% 이상</b>을 권장합니다.</li>
      <li><b>최소 신호수</b>: 가장 중요합니다. <b>20건 이상</b>이면 승률이 어느 정도 안정됩니다. 신호가 부족하면 ②단계에서 <b>심층 조회봉수</b>를 올리거나 보유봉수를 줄이세요.</li>
    </ul>
  </details>
  {#if oosSelectMsg}<div class="hint added">★ {oosSelectMsg}</div>{/if}
  {#if Object.keys($symbolStrategies).length}
    <div class="bt-assign">배정됨:
      {#each $watchlist.filter((c) => $symbolStrategies[c]) as c}
        <span class="assign-chip">{c} → <b>{$symbolStrategies[c].strategy}</b>{#if $symbolStrategies[c].tf}<em>{$symbolStrategies[c].tf}</em>{/if}</span>
      {/each}
    </div>
  {/if}
  {#if $watchlist.length}
    <div class="reflect {!$tradingStatus.running ? 'off' : notReflectedCodes.length ? 'warn' : 'ok'}">
      {#if !$tradingStatus.running}
        ⚪ 자동매매가 <b>정지</b> 상태입니다 — 선정 결과는 저장되었지만,
        <a href="/trading">자동매매 페이지</a>에서 <b>불러오기 → 시작</b>을 눌러야 실제로 반영됩니다.
      {:else if notReflectedCodes.length === 0}
        🟢 실행 중인 자동매매({$tradingStatus.mode === 'live' ? '실전' : '모의'})에 선정한 {$watchlist.length}종목이 모두 반영되어 있습니다.
      {:else}
        🟡 실행 중인 자동매매에 <b>{notReflectedCodes.length}종목이 아직 반영되지 않았습니다</b>: {notReflectedCodes.join(', ')} —
        <a href="/trading">자동매매 페이지</a>에서 불러오기 후 다시 시작하세요.
      {/if}
    </div>
  {/if}
</div>

{#if error}<div class="error">{error}</div>{/if}

<!-- 종목별 × 전략별 백테스트 결과 (OOS 순수익) -->
{#if matrix}
  <div class="panel">
    <h3>
      종목별 · 전략별 백테스트 결과 — {GRID_METRICS[gridMetric].label}
      · {matrix.auto ? 'TF auto(전략별 권장)' : matrix.timeframe} · 보유 {matrix.max_hold_bars}봉
      {#if matrix.history_bars > 0}· 심층 {matrix.history_bars}봉{/if}
    </h3>
    <div class="grid-toolbar">
      <span class="gt-label">셀 표시</span>
      {#each Object.entries(GRID_METRICS) as [k, m]}
        <button type="button" class:on={gridMetric === k} on:click={() => gridMetric = k}>{m.label}</button>
      {/each}
      {#if passPreview != null}
        <span class="pass-preview" class:none={passPreview === 0}>
          현재 기준 통과 <b>{passPreview}</b>종목
        </span>
      {/if}
    </div>
    {#if genSummary.length > 1}
      <div class="gen-summary">
        {#each genSummary as g}
          <div class="gs-card" class:v2={g.gen === 'v2'} class:picked={pickGeneration === g.gen}>
            <div class="gs-head">
              <span class="gs-badge" class:v2={g.gen === 'v2'}>{g.gen === 'v2' ? 'V2' : 'LEGACY'}</span>
              <b>{g.label}</b>
              <span class="gs-n">{g.strategies}전략</span>
            </div>
            <div class="gs-body">
              <span>가중 OOS <b class={g.avgReturn >= 0 ? 'pos' : 'neg'}>{g.avgReturn.toFixed(2)}%</b></span>
              <span>강건성✓ <b>{g.tradeable}</b></span>
              <span>신호 <b>{g.signals}</b></span>
            </div>
            <div class="gs-best">최고: <b>{g.best}</b> {g.bestReturn.toFixed(2)}%</div>
          </div>
        {/each}
      </div>
    {/if}
    <p class="legend">
      <b>굵게</b> = 그 종목의 랭킹기준 1위 전략 · <span class="tick">✓</span> = 강건성 통과 ·
      <span class="v2-chip">보라 열</span> = 신형 V2 전략 ·
      셀에 마우스를 올리면 승률·손익비·손익분기·유지율·권장 보유봉수를 모두 볼 수 있습니다.
      색은 <b>손익분기 대비</b> 기준입니다 (승률 표시일 때 초록 = 손익분기 승률 초과).
      ★는 <b>선정 세대({pickGeneration === 'v2' ? '신형 V2' : '기존 레거시'})</b> 안의 1위 전략을 배정합니다.
    </p>
    <div class="grid-wrap">
      <table class="grid">
        <thead>
          <tr>
            <th class="sticky">종목</th>
            {#each strategyCols as s}
              <th class:v2-col={isV2Strategy(s)}>
                {#if isV2Strategy(s)}<span class="th-gen">V2</span>{/if}
                {s}<span class="th-tf">{tfByStrategy[s] ?? ''}</span>
              </th>
            {/each}
            <th>베스트</th><th>★</th>
          </tr>
        </thead>
        <tbody>
          {#each perSymbolRows as row}
            <tr class:watched={$watchlist.includes(row.code)}>
              <td class="sticky"><strong>{row.name || row.code}</strong><span class="code">{row.code}</span></td>
              {#each strategyCols as s}
                {@const it = row.cells[s]}
                <td class="cell" class:v2-col={isV2Strategy(s)}>
                  {#if it && it.ok}
                    {@const v = GRID_METRICS[gridMetric].get(it)}
                    <span class={v != null && cellPositive(it, v) ? 'pos' : 'neg'} class:bestcell={row.best === s}
                      class:picked={passesCriteria(it)} title={cellTitle(it)}>
                      {v == null ? '-' : GRID_METRICS[gridMetric].fmt(v)}{it.tradeable ? ' ✓' : ''}
                      <em class="cell-n">{it.oos_total_signals ?? 0}</em>
                    </span>
                  {:else}<span class="na">-</span>{/if}
                </td>
              {/each}
              <td>{row.best ? row.best : '—'}</td>
              <td>
                <button class="star" class:on={$watchlist.includes(row.code)}
                  disabled={!$watchlist.includes(row.code) && !row.bestOfPick}
                  on:click={() => {
                    watchlist.toggle(row.code);
                    if (row.bestOfPick) symbolStrategies.setOne(row.code, { strategy: row.bestOfPick, tf: row.cells[row.bestOfPick]?.tf });
                  }}
                  title={row.bestOfPick
                    ? `관심종목 추가/제거 (추가 시 ${row.bestOfPick} 배정)`
                    : `${pickGeneration === 'v2' ? '신형 V2' : '기존 레거시'} 전략 중 거래가 발생한 것이 없어 배정할 수 없습니다`}
                  >{$watchlist.includes(row.code) ? '★' : '☆'}</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>
{:else if scanned}
  <div class="panel empty">가격 필터로 후보 {candidates.length}종목을 선별했습니다. <b>② 백테스트</b>를 실행하면 종목별·전략별 OOS 순수익이 표시됩니다.</div>
{/if}

<style>
  header h1 { margin: 0 0 4px; font-size: 22px; }
  .sub { color: #a6adc8; margin: 0 0 16px; font-size: 13px; }
  .sub code { background: #313244; padding: 1px 5px; border-radius: 4px; color: #bac2de; }
  .picker-wrap { margin-bottom: 14px; }
  .controls { display: flex; gap: 16px; align-items: flex-end; margin-bottom: 16px; flex-wrap: wrap; }
  .group { display: flex; flex-direction: column; gap: 6px; }
  label { font-size: 12px; color: #bac2de; }
  select, input[type='number'] { background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 5px 8px; width: 110px; }
  .refresh { background: #89b4fa; color: #1e1e2e; border: none; border-radius: 6px; padding: 8px 16px; font-weight: 600; cursor: pointer; }
  .hint { font-size: 13px; color: #f9e2af; background: #1f1d2e; border: 1px solid #45475a; border-radius: 6px; padding: 8px 12px; margin: 10px 0 0; }
  .hint.added { color: #a6e3a1; }
  .step { background: #181825; border: 1px solid #313244; border-radius: 10px; padding: 12px 14px; margin-bottom: 12px; }
  .step-head { font-size: 14px; color: #cdd6f4; margin-bottom: 10px; }
  .step-no { color: #89b4fa; font-weight: 700; }
  .step-sub { color: #a6adc8; font-size: 12px; font-weight: 400; }
  .step .controls { margin-bottom: 0; }
  .cand-count { font-size: 13px; color: #cdd6f4; align-self: center; }
  .cand-count b { color: #a6e3a1; }
  .bt-btn { background: #cba6f7; color: #1e1e2e; border: none; border-radius: 6px; padding: 8px 16px; font-weight: 600; cursor: pointer; }
  .bt-btn.pick { background: #a6e3a1; }
  .bt-btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .chk { display: flex; align-items: center; gap: 6px; font-size: 13px; color: #cdd6f4; align-self: center; cursor: pointer; white-space: nowrap; }
  .chk input { width: auto; }
  .dflt { color: #a6adc8; font-size: 11px; font-weight: 400; }
  .bt-note { font-size: 12px; color: #a6adc8; flex: 1; min-width: 240px; line-height: 1.5; }
  .bt-assign { margin-top: 10px; display: flex; gap: 8px; flex-wrap: wrap; font-size: 12px; color: #bac2de; }
  .assign-chip { background: #1e1e2e; border: 1px solid #313244; border-radius: 10px; padding: 2px 8px; }
  .assign-chip b { color: #cba6f7; }
  .assign-chip em { color: #a6adc8; font-style: normal; font-size: 11px; margin-left: 4px; }
  .reflect { margin-top: 10px; font-size: 12px; padding: 8px 12px; border-radius: 6px; border: 1px solid #45475a; }
  .reflect a { color: inherit; text-decoration: underline; }
  .reflect.off { color: #bac2de; background: #1e1e2e; }
  .reflect.ok { color: #a6e3a1; background: #1a2a1a; border-color: #3a4a3a; }
  .reflect.warn { color: #f9e2af; background: #2a2717; border-color: #4a4530; }
  .to-trading { margin-left: auto; color: #a6e3a1; text-decoration: none; font-size: 13px; align-self: center; }
  .panel { background: #181825; border-radius: 10px; padding: 12px 16px; margin-top: 4px; }
  .panel.empty { color: #bac2de; font-size: 13px; }
  .panel h3 { margin: 0 0 6px; font-size: 15px; }
  .legend { color: #a6adc8; font-size: 12px; margin: 0 0 12px; }
  .error { background: #f38ba8; color: #1e1e2e; padding: 10px; border-radius: 6px; margin-bottom: 12px; font-size: 13px; }
  /* 종목 × 전략 OOS 격자 */
  .grid-wrap { overflow-x: auto; }
  table.grid { width: 100%; border-collapse: collapse; font-size: 12px; }
  table.grid th { text-align: right; padding: 6px 8px; color: #bac2de; border-bottom: 1px solid #313244; font-weight: 500; white-space: nowrap; }
  table.grid th:first-child { text-align: left; }
  table.grid td { padding: 6px 8px; border-bottom: 1px solid #232334; text-align: right; font-variant-numeric: tabular-nums; }
  table.grid td:first-child { text-align: left; }
  .th-tf { display: block; font-size: 10px; color: #a6adc8; font-weight: 400; }
  .sticky { position: sticky; left: 0; background: #181825; z-index: 1; }
  tr.watched .sticky { background: #1a1a2a; }
  .code { color: #a6adc8; margin-left: 6px; font-size: 11px; }
  .cell .bestcell { font-weight: 700; background: #1d2a1d; padding: 1px 5px; border-radius: 4px; }
  /* 현재 선정 기준을 통과한 셀 — 실제로 뽑히는 후보를 한눈에 */
  .cell .picked { outline: 1px solid #89b4fa; outline-offset: 1px; border-radius: 3px; }
  .cell .cell-n { font-size: 9.5px; color: #6c7086; font-style: normal; margin-left: 3px; }

  /* 셀 표시 전환 툴바 */
  .grid-toolbar { display: flex; align-items: center; gap: 5px; margin: 0 0 8px; flex-wrap: wrap; }
  .gt-label { font-size: 12px; color: #a6adc8; margin-right: 3px; }
  .grid-toolbar button { background: #1e1e2e; color: #a6adc8; border: 1px solid #313244; border-radius: 4px;
                         padding: 3px 9px; font-size: 11.5px; cursor: pointer; }
  .grid-toolbar button:hover { border-color: #585b70; color: #cdd6f4; }
  .grid-toolbar button.on { background: #313244; color: #cdd6f4; border-color: #89b4fa; font-weight: 600; }
  .pass-preview { margin-left: auto; font-size: 12px; color: #a6e3a1; background: #1a2a1f;
                  border-radius: 4px; padding: 3px 9px; }
  .pass-preview.none { color: #f9e2af; background: #2a2416; }

  /* ③단계 승률 기준 서브 컨트롤 */
  .sub-controls { border-top: 1px dashed #313244; padding-top: 12px; }
  .grp-title { font-size: 12px; font-weight: 600; color: #f9e2af; align-self: center; padding-bottom: 6px; }

  /* ---------- 전략 세대 선택 ---------- */
  .gen-lead {
    background: #211d2e; border-left: 3px solid #cba6f7; border-radius: 0 6px 6px 0;
    padding: 8px 12px; margin-top: 8px; line-height: 1.6;
  }
  .gen-row { align-items: center; gap: 12px; }
  .gen-seg { display: flex; gap: 4px; align-self: center; padding-bottom: 6px; }
  .gen-seg button {
    padding: 6px 14px; border: 1px solid #45475a; background: #1e1e2e; color: #cdd6f4;
    border-radius: 5px; cursor: pointer; font-size: 12.5px; white-space: nowrap;
  }
  .gen-seg button.on { background: #89b4fa; color: #1e1e2e; font-weight: 700; border-color: #89b4fa; }
  .gen-seg button.on.v2 { background: #cba6f7; border-color: #cba6f7; }
  .gen-seg button:disabled { opacity: .35; cursor: not-allowed; }
  .gen-hint { font-size: 11.5px; color: #a6adc8; line-height: 1.55; flex: 1 1 260px; padding-bottom: 6px; }
  .preset-btns { display: flex; gap: 6px; align-self: center; padding-bottom: 6px; }
  .crit-preset {
    background: #313244; color: #cdd6f4; border: none; border-radius: 5px;
    padding: 6px 12px; font-size: 12px; cursor: pointer; white-space: nowrap;
  }
  .crit-preset.v2 { background: #cba6f7; color: #1e1e2e; font-weight: 700; }
  .muted-row { opacity: .5; }
  .overshoot-warn {
    font-size: 11.5px; color: #f9e2af; background: #2a2717; border-radius: 5px;
    padding: 6px 10px; line-height: 1.5; flex: 1 1 320px; align-self: center;
  }
  .bt-btn.v2 { background: #cba6f7; }
  .mkt-caveat {
    margin-top: 10px; font-size: 11.5px; color: #cba6f7; background: #211d2e;
    border-radius: 6px; padding: 8px 12px; line-height: 1.6;
  }

  /* ---------- 세대별 집계 카드 ---------- */
  .gen-summary { display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px; margin-bottom: 12px; }
  .gs-card { background: #1e1e2e; border: 1px solid #313244; border-radius: 8px; padding: 10px 12px; }
  .gs-card.picked { border-color: #89b4fa; }
  .gs-card.v2.picked { border-color: #cba6f7; }
  .gs-head { display: flex; align-items: center; gap: 7px; font-size: 13px; }
  .gs-badge {
    font-size: 9.5px; font-weight: 800; letter-spacing: .05em; padding: 2px 5px;
    border-radius: 3px; background: #45475a; color: #bac2de;
  }
  .gs-badge.v2 { background: #cba6f7; color: #1e1e2e; }
  .gs-n { font-size: 11px; color: #a6adc8; margin-left: auto; }
  .gs-body { display: flex; gap: 14px; font-size: 11.5px; color: #a6adc8; margin-top: 6px; }
  .gs-body b { color: #cdd6f4; font-variant-numeric: tabular-nums; }
  .gs-best { font-size: 11px; color: #a6adc8; margin-top: 4px; }
  @media (max-width: 900px) { .gen-summary { grid-template-columns: 1fr; } }

  /* v2 전략 열은 배경으로 구분 — 두 세대를 나란히 볼 때 혼동을 막는다. */
  table.grid th.v2-col, table.grid td.v2-col { background: #221d30; }
  table.grid th.v2-col { color: #cba6f7; }
  .th-gen {
    display: block; font-size: 8.5px; font-weight: 800; letter-spacing: .06em;
    color: #cba6f7; margin-bottom: 1px;
  }
  .v2-chip {
    background: #221d30; color: #cba6f7; padding: 1px 5px; border-radius: 3px; font-weight: 600;
  }
  .star:disabled { opacity: .3; cursor: not-allowed; }
  .crit-help { margin: 6px 0 0; font-size: 12px; color: #a6adc8; }
  .crit-help summary { cursor: pointer; color: #89b4fa; }
  .crit-help ul { margin: 8px 0 0; padding-left: 18px; line-height: 1.7; }
  .crit-help code { background: #313244; padding: 1px 5px; border-radius: 4px; }
  .warn-inline { color: #f9e2af; }
  .universe-age { font-size: 12px; border-radius: 5px; padding: 7px 10px; margin: -8px 0 14px; line-height: 1.6; }
  .universe-age.fresh { background: #1a2a1f; color: #a6adc8; border-left: 3px solid #a6e3a1; }
  .universe-age.stale { background: #2a2416; color: #f9e2af; border-left: 3px solid #f9e2af; }
  .universe-age code { background: #00000033; padding: 1px 5px; border-radius: 4px; }
  input.narrow { width: 80px; }
  .na { color: #45475a; }
  .pos { color: #a6e3a1; } .neg { color: #f38ba8; }
  .tick { color: #a6e3a1; }
  .star { background: none; border: none; cursor: pointer; font-size: 15px; color: #a6adc8; padding: 0; }
  .star.on { color: #f9e2af; }

  /* eBest API 패널 */
  .api-panel { background: #181825; border: 1px solid #313244; border-radius: 10px; padding: 12px 14px; margin-bottom: 14px; }
  .api-head { display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap; }
  .api-title { display: flex; align-items: center; gap: 8px; font-size: 14px; flex-wrap: wrap; }
  .toggle { background: none; border: none; color: #bac2de; cursor: pointer; font-size: 13px; padding: 0; }
  .badge { font-size: 11px; font-weight: 700; padding: 2px 8px; border-radius: 10px; }
  .badge.mock { background: #f9e2af; color: #1e1e2e; }
  .badge.live { background: #a6e3a1; color: #1e1e2e; }
  .badge.ok { background: #313244; color: #a6e3a1; }
  .badge.off { background: #313244; color: #f38ba8; }
  .api-actions { display: flex; gap: 8px; align-items: center; }
  .code-in { background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 5px 8px; width: 100px; }
  .test-btn { background: #cba6f7; color: #1e1e2e; border: none; border-radius: 6px; padding: 7px 14px; font-weight: 700; cursor: pointer; }
  .test-btn:disabled { opacity: 0.5; cursor: progress; }
  .api-result { margin-top: 12px; }
  .rsum { font-size: 13px; margin-bottom: 8px; font-weight: 600; }
  .rsum.ok { color: #a6e3a1; }
  .rsum.fail { color: #f38ba8; }
  .api-result table { width: 100%; border-collapse: collapse; font-size: 12px; }
  .api-result th { text-align: left; padding: 5px 7px; color: #bac2de; border-bottom: 1px solid #313244; font-weight: 500; }
  .api-result td { padding: 5px 7px; border-bottom: 1px solid #232334; }
  .api-result td.ok { color: #a6e3a1; }
  .api-result td.fail { color: #f38ba8; }
  .api-result td.tr { color: #a6adc8; font-family: monospace; }
  .api-result td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .api-result td.detail { color: #bac2de; }
  .api-hint { font-size: 12px; color: #a6adc8; margin: 10px 0 0; }
  .api-hint code { background: #313244; padding: 1px 5px; border-radius: 4px; color: #bac2de; }

  /* 보유봉수 자유 입력 + 프리셋 */
  .hold-input { display: flex; flex-direction: column; gap: 5px; }
  .hold-input input { width: 96px; background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a;
                      border-radius: 4px; padding: 6px 8px; font-variant-numeric: tabular-nums; }
  .presets { display: flex; gap: 3px; flex-wrap: wrap; }
  .presets button { background: #1e1e2e; color: #a6adc8; border: 1px solid #313244; border-radius: 4px;
                    padding: 2px 7px; font-size: 11px; cursor: pointer; }
  .presets button:hover { border-color: #585b70; color: #cdd6f4; }
  .presets button.on { background: #313244; color: #cdd6f4; border-color: #89b4fa; font-weight: 600; }

  /* OOS 표본 용량 배너 */
  .capacity { border-radius: 6px; padding: 10px 12px; margin: 8px 0 0; font-size: 12.5px; line-height: 1.6; }
  .capacity.ok { background: #1a2a1f; border-left: 3px solid #a6e3a1; color: #cdd6f4; }
  .capacity.bad { background: #2a2416; border-left: 3px solid #f9e2af; color: #f9e2af; }
  .cap-head { font-size: 13px; }
  .cap-body { margin-top: 4px; display: flex; gap: 6px; flex-wrap: wrap; }
  .cap-chip { background: #00000033; border-radius: 4px; padding: 2px 7px; font-size: 11.5px; color: #a6adc8; }
  .cap-warn { margin-top: 6px; padding-top: 6px; border-top: 1px solid #ffffff1a; }
</style>
