<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { api, type ReadinessReport, type Timeframe, type TradeEvent, type TradeStats } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';
  import { symbolStrategies } from '$lib/stores/symbolStrategies';
  import { loadTradingSettings, saveTradingSettings } from '$lib/stores/tradingSettings';
  import ReadinessPanel from '$lib/components/ReadinessPanel.svelte';
  import ActiveTradeCharts from '$lib/components/ActiveTradeCharts.svelte';

  // 이전에 저장한 설정을 기본값으로 불러온다 (없으면 하드코딩 기본값).
  const saved = loadTradingSettings();

  let report: ReadinessReport | null = null;
  let mode = saved.mode ?? 'paper';
  let strategy = saved.strategy ?? 'balanced';
  let tf: Timeframe = (saved.tf as Timeframe) ?? '5m';
  const initialWatch = get(watchlist);
  let watchlistText = saved.watchlistText ?? (initialWatch.length ? initialWatch.join(', ') : '005930, 000660, 035420');
  let pollSec = saved.pollSec ?? 60;
  // 기본 false — 장시간 무시가 기본값이면 주말/장외에 스테일 봉으로 엔진이 돌아
  // 금요일 마지막 봉을 새 봉으로 오인하는 진입이 생길 수 있다 (테스트 시에만 수동 ON).
  // 실전 모드에서는 항상 강제 해제 (저장값이 live+ON 조합이어도).
  let ignoreHours = (saved.mode ?? 'paper') === 'live' ? false : (saved.ignoreHours ?? false);

  // 주문 설정
  let orderType = saved.orderType ?? 'limit';          // limit=지정가 | market=시장가 | best=최유리지정가
  let fixedQty = saved.fixedQty ?? 0;                  // 1회 매수/매도 수량 (0=자동 산정)
  let sellAll = saved.sellAll ?? true;                 // 매도 시 전량
  let maxPositions = saved.maxPositions ?? 5;          // 동시 보유 최대 종목 수 (분산 5종목이 일반적)
  // 매수 한도액 = 총 진입금액(보유 포지션 전체 합계) 한도. 모의/실전 모드별로 값을
  // 분리 저장한다 — 실전은 잔고금액을, 모의는 이 값을 기본값으로 쓴다.
  let maxBuyAmount = saved.maxBuyAmount ?? 500000;         // 모의투자용
  let maxBuyAmountLive = saved.maxBuyAmountLive ?? 0;      // 실전투자용 (0 = 아직 계좌 잔고로 초기화 전)
  let liveBalanceLoading = false;
  // 실전 모드로 처음 진입할 때 잔고금액을 조회해 기본값으로 채운다. 이미 값이
  // 있으면(사용자가 조정했거나 이전에 조회됨) 덮어쓰지 않는다.
  async function ensureLiveBalanceDefault(force = false) {
    if (liveBalanceLoading || (!force && maxBuyAmountLive > 0)) return;
    liveBalanceLoading = true;
    try {
      const { balance } = await api.accountBalance();
      // 기본값은 잔고의 50%만 — 전액을 한도로 잡으면 첫 실행부터 계좌 전체가
      // 리스크에 노출된다. 사용자가 원하면 직접 올릴 수 있다.
      if (balance > 0) maxBuyAmountLive = Math.floor(balance * 0.5);
    } catch {
      // 계좌 조회 실패(키 미설정 등) — 조용히 무시, 사용자가 직접 입력 가능
    } finally {
      liveBalanceLoading = false;
    }
  }
  function selectMode(m: string) {
    mode = m;
    if (m === 'live') {
      ensureLiveBalanceDefault();
      // 실전에서 장시간 무시는 스테일 봉 기반 실주문으로 이어진다 — 강제 해제.
      ignoreHours = false;
    }
  }
  $: activeMaxBuyAmount = mode === 'live' ? maxBuyAmountLive : maxBuyAmount;

  // 손절·익절 설정 (수동 고정%; 0 = ATR 변동성 기준 자동)
  // 기본 −3% / +6% (손익비 2:1) — 국내 단타에서 가장 널리 쓰는 조합.
  let stopLossPct = saved.stopLossPct ?? 3;            // 매수가 대비 손절 % (예: 3 → -3%)
  let takeProfitPct = saved.takeProfitPct ?? 6;        // 매수가 대비 익절 % (예: 6 → +6%)

  // 자동 매도(청산) 조건 — 각 트리거를 개별 on/off
  let useStopLoss = saved.useStopLoss ?? true;         // 손절선 도달 시 청산
  let useTakeProfit = saved.useTakeProfit ?? true;     // 익절선 도달 시 청산
  let useTrailingStop = saved.useTrailingStop ?? true; // 고점 대비 되돌림 시 청산
  let trailingStopAtr = saved.trailingStopAtr ?? 2.0;  // 트레일링 ATR 배수

  // 재진입 가드 (whipsaw 방지) + 피보나치 평균매수
  let lossCooldownBars = saved.lossCooldownBars ?? 3;       // 손절 청산 후 진입 금지 봉수
  let reentryCooldownBars = saved.reentryCooldownBars ?? 1; // 익절 청산 후 진입 금지 봉수
  let reentryGapPct = saved.reentryGapPct ?? 1;             // 손절가 × (1-이값) 이하에서만 재매수 (%) — 1%가 일반적
  let reentryGuardExpireBars = saved.reentryGuardExpireBars ?? 20; // 가격가드 자동 해제 봉수 (0=무기한)
  let fibEnabled = saved.fibEnabled ?? false;              // 피보나치 평균매수(물타기) 사용
  // 기본 2차 권장 — 3차부터 추가수량(피보나치 1·1·2·3·5)이 급증해 하락장 손실이
  // 기하급수로 커진다. 백엔드도 최대 5차로 강제 제한한다.
  let fibMaxLevels = saved.fibMaxLevels ?? 2;              // 물타기 최대 차수

  // 진입 품질 게이트 + 청산 안정화 + OOS 선별
  let requireConfirmation = saved.requireConfirmation ?? true;        // 확인봉(양봉/고점돌파)에서만 진입
  let confirmWindowBars = saved.confirmWindowBars ?? 3;               // 패턴 후 이 봉수 안의 확인도 인정
  let requireHigherTfUptrend = saved.requireHigherTfUptrend ?? true;  // 상위 TF 하락 시 롱 진입 금지
  let higherTfTolerancePct = saved.higherTfTolerancePct ?? 0;         // 허용 하락 기울기 %/봉 (0=엄격)
  let minHoldBars = saved.minHoldBars ?? 1;                  // 익절/트레일링 최소 보유봉수
  // 실시간 손절 기본 ON + 버퍼 0.5% — 봉 마감 전 급락을 놓치지 않으면서
  // 틱 노이즈로 인한 조기 손절은 버퍼로 거른다 (단타 표준 구성).
  let hardStopIntrabar = saved.hardStopIntrabar ?? true;     // 형성 중 봉 실시간가로 손절(off=닫힌 봉)
  let hardStopBufferPct = saved.hardStopBufferPct ?? 0.5;    // intrabar 손절 버퍼(%)
  let eodFlatten = saved.eodFlatten ?? true;                 // 장 마감 전 강제 청산(오버나이트 갭 방지)
  let requireTradeable = saved.requireTradeable ?? true;     // OOS 검증 통과 종목만 매매

  function loadFromWatchlist() {
    const codes = get(watchlist);
    if (codes.length) watchlistText = codes.join(', ');
  }

  // 대시보드(OOS 순수익 기준 선정)에서 정한 관심종목이 이 페이지의 입력값과 다른지 확인한다.
  // watchlistText는 별도로 저장되는 값이라 대시보드에서 새로 선정해도 자동 반영되지 않으므로,
  // "불러오기"를 눌러야 함을 눈에 띄게 알려준다.
  $: watchlistCodes = watchlistText.split(',').map((s) => s.trim()).filter(Boolean);
  $: watchlistStale = $watchlist.length > 0 &&
    ($watchlist.length !== watchlistCodes.length || $watchlist.some((c) => !watchlistCodes.includes(c)));

  let status: any = { running: false, positions: [], daily_pnl: 0, trade_events: [] };
  let presets: Record<string, any> = {};
  // 가중치/임계는 프리셋에서 채워지지만, 사용자가 조정한 값이 저장돼 있으면 그대로 복원한다.
  let weights: Record<string, number> = saved.weights ? { ...saved.weights } : {};
  let entryThreshold = saved.entryThreshold ?? 0.65;
  const hasSavedWeights = !!saved.weights;
  let error = '';
  let info = ''; // 오류가 아닌 안내(예: OOS 제외 종목) — 에러 박스와 분리 표시
  let timer: ReturnType<typeof setInterval>;

  // 거래 내역 / 통계
  let journalTrades: any[] = [];
  let tradeStats: TradeStats | null = null;
  let journalMode = 'paper';

  // 보유 포지션 현재가 실시간 폴링 (엔진 폴링 주기와 무관하게 5초마다 갱신)
  let livePrices: Record<string, number> = {};
  let liveAt = '';

  const sources = ['rule', 'atr', 'volume', 'mtf', 'ml', 'gaf'];

  async function loadPresets() {
    presets = await api.presets();
    // 저장된 가중치가 있으면 사용자 값을 유지하고, 없을 때만 프리셋 기본값을 적용한다.
    if (!hasSavedWeights) applyPreset();
  }
  function applyPreset() {
    const p = presets[strategy];
    if (!p) return;
    weights = { ...p.weights };
    entryThreshold = p.entry_threshold;
  }
  // 전략을 사용자가 바꿀 때만 해당 프리셋 가중치를 새로 적용한다(초기 로드 시엔 저장값 우선).
  function onStrategyChange() {
    applyPreset();
  }

  // 설정값이 바뀔 때마다 localStorage에 저장 → 다음 방문 시 기본값으로 유지된다.
  $: saveTradingSettings({
    mode, strategy, tf, watchlistText, pollSec, ignoreHours,
    orderType, fixedQty, sellAll, maxPositions, maxBuyAmount, maxBuyAmountLive,
    stopLossPct, takeProfitPct,
    useStopLoss, useTakeProfit, useTrailingStop, trailingStopAtr,
    lossCooldownBars, reentryCooldownBars, reentryGapPct, reentryGuardExpireBars,
    fibEnabled, fibMaxLevels,
    requireConfirmation, confirmWindowBars, requireHigherTfUptrend, higherTfTolerancePct,
    minHoldBars, hardStopIntrabar, hardStopBufferPct,
    eodFlatten, requireTradeable, weights, entryThreshold,
  });

  async function refreshStatus() {
    try {
      status = await api.tradingStatus();
      await refreshLivePrices();
    } catch (e) { error = String(e); }
  }
  async function refreshReadiness() {
    try { report = await api.readiness(); } catch (e) { /* ignore */ }
  }
  async function refreshJournal() {
    try {
      [journalTrades, tradeStats] = await Promise.all([
        api.journal(journalMode, 200),
        api.tradeStats(journalMode),
      ]);
    } catch {}
  }

  // 보유 포지션의 현재가를 실시간 갱신.
  // 진입(매수)이 이뤄진 것과 '동일한 타임프레임' 종가로 비교해야 미실현손익이
  // 실제 매수가 기준으로 일관되게 계산된다.
  async function refreshLivePrices() {
    const codes: string[] = (status.positions ?? []).map((p: any) => p.code);
    if (!codes.length) { livePrices = {}; return; }
    const ptf = (status.timeframe ?? tf) as Timeframe;
    try {
      const qs = await Promise.all(codes.map((c) => api.quote(c, ptf).catch(() => null)));
      const next: Record<string, number> = {};
      for (const q of qs) if (q) next[q.shcode] = q.price;
      livePrices = next;
      liveAt = new Date().toLocaleTimeString('ko-KR');
    } catch {}
  }

  // 포지션별 실시간 현재가/미실현손익 — livePrices 우선, 없으면 엔진 값.
  // 숏 포지션은 가격이 내려야 수익이므로 방향(dir)을 반영한다.
  function liveCur(p: any): number {
    return livePrices[p.code] ?? p.current_price ?? p.entry;
  }
  function posDir(p: any): number {
    return p.side === 'short' ? -1 : 1;
  }
  function liveUpnl(p: any): number {
    return (liveCur(p) - p.entry) * p.qty * posDir(p);
  }
  function liveUpct(p: any): number {
    return p.entry ? ((liveCur(p) - p.entry) / p.entry) * 100 * posDir(p) : 0;
  }
  // 평균매수단가 (물타기 시 수량가중 평균으로 갱신됨)
  function avgPrice(p: any): number {
    return p.buy_price ?? p.entry;
  }
  // 총 매수금액 = 평균매수단가 × 수량
  function totalBuy(p: any): number {
    return avgPrice(p) * p.qty;
  }
  // 총 평가금액 = 현재가 × 수량
  function totalEval(p: any): number {
    return liveCur(p) * p.qty;
  }
  // 실시간 평가자산/미실현 합계 — 백엔드 equity()와 동일하게 숏은 미실현손익만 가산
  // (숏 진입은 현금을 쓰지도 받지도 않으므로 qty×현재가로 더하면 자산이 부풀려진다).
  $: positions = status.positions ?? [];
  $: liveEquity =
    (status.cash ?? 0) +
    positions.reduce(
      (s: number, p: any) => s + (p.side === 'short' ? (p.entry - liveCur(p)) * p.qty : p.qty * liveCur(p)),
      0
    );
  $: liveUnrealTotal = positions.reduce((s: number, p: any) => s + liveUpnl(p), 0);

  // 보유 포지션 합계 (수량·총매수금액·총평가금액·미실현손익) — 표 하단 합계 행/강조 박스에 사용.
  $: totalQty = positions.reduce((s: number, p: any) => s + p.qty, 0);
  $: totalBuyAmt = positions.reduce((s: number, p: any) => s + totalBuy(p), 0);
  $: totalEvalAmt = positions.reduce((s: number, p: any) => s + totalEval(p), 0);
  $: totalUpct = totalBuyAmt ? (liveUnrealTotal / totalBuyAmt) * 100 : 0;

  // 총매수금액 합계 vs 매수 한도액(총 진입금액 한도) — 실행 중이면 엔진에 적용된 한도, 아니면 설정 폼의 값을 기준으로 비교.
  $: buyLimit = cfgOrder?.max_buy_amount ?? activeMaxBuyAmount;
  $: buyLimitDiff = totalBuyAmt - buyLimit;
  $: buyLimitRatio = buyLimit > 0 ? (totalBuyAmt / buyLimit) * 100 : 0;

  let closing: Record<string, boolean> = {};
  async function closePosition(p: any) {
    const live = status.mode === 'live';
    const closeWord = p.side === 'short' ? '청산(매수 환매)' : '청산(매도)';
    const msg = live
      ? `[실전] ${p.name || p.code} ${p.qty}주를 실제 시장가/지정가로 청산 주문합니다. 계속할까요?`
      : `${p.name || p.code} ${p.qty}주를 현재가로 ${closeWord}합니다. 계속할까요?`;
    if (!confirm(msg)) return;
    closing = { ...closing, [p.code]: true };
    try {
      await api.closePosition(p.code);
      await refreshStatus();
      await refreshJournal();
    } catch (e) {
      error = String(e);
    } finally {
      closing = { ...closing, [p.code]: false };
    }
  }

  let telegramSending = false;
  async function sendTelegram() {
    telegramSending = true;
    try {
      await api.telegramReport();
      alert('stock_monitor 텔레그램 방으로 전송했습니다.');
    } catch (e) {
      error = String(e);
      alert('텔레그램 전송 실패: ' + String(e));
    } finally {
      telegramSending = false;
    }
  }
  async function clearSessionEvents() {
    if (!confirm('이번 세션 거래 기록과 차트의 매수/매도 마커를 모두 지웁니다. 계속할까요?')) return;
    try { await api.clearEvents(); await refreshStatus(); } catch (e) { error = String(e); }
  }
  async function clearJournalRecords() {
    const label = journalMode === 'all' ? '전체' : journalMode === 'live' ? '실전' : '모의';
    if (!confirm(`${label} 누적 거래내역을 영구 삭제합니다. 되돌릴 수 없습니다. 계속할까요?`)) return;
    try { await api.clearJournal(journalMode); await refreshJournal(); } catch (e) { error = String(e); }
  }

  $: journalMode, refreshJournal();

  $: liveAdvisory = mode === 'live' && report != null && !report.ready;

  // 차트·보조지표 대상 = 엔진이 실제로 스캔하는 집합(워치리스트 ∪ 보유포지션)과 일치시킨다.
  // 백엔드 scan_and_trade 및 모니터링 현황(status.monitor)이 쓰는 종목 집합과 동일하게 맞춰
  // 세 영역(관심종목·모니터링·차트)의 종목 리스트 불일치를 없앤다.
  $: activeCodes = status.running
    ? Array.from(
        new Set<string>([
          ...((status.watchlist ?? []) as string[]),
          ...((status.positions ?? []) as any[]).map((p) => p.code as string),
        ])
      )
    : watchlistCodes;

  $: tradeEvents = (status.trade_events ?? []) as TradeEvent[];

  // 워치리스트에는 없지만 보유 중이라 계속 모니터링되는 종목(설정 표시용).
  $: extraHeldCodes = ((status.positions ?? []) as any[])
    .map((p) => p.code as string)
    .filter((c) => !((status.watchlist ?? []) as string[]).includes(c));

  async function start(confirmDiscard = false) {
    error = '';
    info = '';
    if (mode === 'live' && !report?.ready) {
      const ok = confirm('검증 기준 미달 상태입니다. 그래도 실전투자를 시작하시겠습니까?\n(권장: 모의투자로 충분히 검증 후 진행)');
      if (!ok) return;
    }
    try {
      const wl = watchlistText.split(',').map((s: string) => s.trim()).filter(Boolean);
      // 대시보드의 백테스트 선정에서 배정된 종목별 전략을 함께 전달 (배정 없는 종목은 전역 전략 사용).
      // 배정에 저장된 TF(백테스트에 실제 쓰인 타임프레임)를 recommended_tf 로 넘겨,
      // 자동매매가 백테스트와 동일한 TF 로 그 전략을 돌리도록 한다.
      const assigned = get(symbolStrategies);
      const symMap: Record<string, { name: string; recommended_tf?: string }> = {};
      for (const c of wl) {
        const a = assigned[c];
        if (a) symMap[c] = a.tf ? { name: a.strategy, recommended_tf: a.tf } : { name: a.strategy };
      }
      const resStart = await api.tradingStart({
        mode, strategy: { name: strategy, weights, entry_threshold: entryThreshold },
        symbol_strategies: symMap,
        watchlist: wl, tf, poll_sec: pollSec, ignore_market_hours: ignoreHours,
        confirm_discard: confirmDiscard,
        order: {
          order_type: orderType,
          fixed_qty: fixedQty,
          sell_all: sellAll,
          max_buy_amount: activeMaxBuyAmount,
        },
        require_tradeable: requireTradeable,
        risk: {
          stop_loss_pct: stopLossPct / 100,
          take_profit_pct: takeProfitPct / 100,
          use_stop_loss: useStopLoss,
          use_take_profit: useTakeProfit,
          use_trailing_stop: useTrailingStop,
          trailing_stop_atr: trailingStopAtr,
          loss_cooldown_bars: lossCooldownBars,
          reentry_cooldown_bars: reentryCooldownBars,
          reentry_gap_pct: reentryGapPct / 100,
          reentry_guard_expire_bars: reentryGuardExpireBars,
          max_positions: maxPositions,
          fib_averaging_enabled: fibEnabled,
          fib_max_levels: fibMaxLevels,
          require_confirmation: requireConfirmation,
          confirm_window_bars: confirmWindowBars,
          require_higher_tf_uptrend: requireHigherTfUptrend,
          higher_tf_slope_tolerance: higherTfTolerancePct / 100,
          min_hold_bars: minHoldBars,
          hard_stop_intrabar: hardStopIntrabar,
          hard_stop_buffer_pct: hardStopBufferPct / 100,
          eod_flatten: eodFlatten,
        },
      });
      const notes: string[] = [];
      if (resStart?.dropped_untradeable?.length) {
        notes.push(`OOS 미통과로 제외된 종목: ${resStart.dropped_untradeable.join(', ')}`);
      }
      if (resStart?.restored_from_snapshot) {
        notes.push('이전 세션의 보유 포지션/현금을 스냅샷에서 복원했습니다.');
      }
      info = notes.join(' · ');
      await refreshStatus();
      await refreshReadiness();
    } catch (e) {
      const msg = String(e);
      // 이전 엔진(모드 전환 등)에 보유 포지션이 있으면 백엔드가 확인을 요구한다.
      const m = msg.match(/DISCARD_CONFIRM:(.*)$/s);
      if (m && !confirmDiscard) {
        if (confirm(`${m[1].trim()}\n\n계속 진행할까요?`)) await start(true);
        return;
      }
      error = msg;
    }
  }
  async function stop() {
    try { await api.tradingStop(); await refreshStatus(); await refreshJournal(); } catch (e) { error = String(e); }
  }
  async function pushStrategy() {
    try { await api.updateStrategy({ name: strategy, weights, entry_threshold: entryThreshold }); } catch (e) { error = String(e); }
  }

  onMount(() => {
    loadPresets(); refreshStatus(); refreshReadiness(); refreshJournal();
    if (mode === 'live') ensureLiveBalanceDefault();
    timer = setInterval(() => { refreshStatus(); refreshReadiness(); }, 5000);
  });
  onDestroy(() => clearInterval(timer));

  // 세션 거래 이벤트의 구분 라벨: 진입/청산 × 롱/숏.
  // 숏은 진입이 매도(신규매도), 청산이 매수(환매)라서 type(매수/매도)만 보여주면
  // 보유 포지션과 매칭이 안 된다 — action/side 로 명확히 표기한다.
  function evLabel(ev: TradeEvent): string {
    if (!ev.action) return ev.type === 'buy' ? '매수' : '매도'; // 물타기 등 구분 없는 이벤트
    if (ev.side === 'short') return ev.action === 'open' ? '숏 진입(매도)' : '숏 청산(매수)';
    return ev.action === 'open' ? '롱 진입(매수)' : '롱 청산(매도)';
  }
  // 실현손익은 '청산' 이벤트에만 있다. 숏 청산은 type=buy 이므로 type 으로
  // 판단하면 숏의 실현손익이 숨고 숏 진입에 0원이 표시된다.
  function evIsClose(ev: TradeEvent): boolean {
    return ev.action ? ev.action === 'close' : ev.type === 'sell';
  }
  // 구분 컬럼 색상: 진입(open)만 매수(빨강)/매도(파랑)로 칠하고, 청산(close)은
  // 방향이 아니라 손익 결과(초록/분홍)로 칠한다. 진입·청산에 같은 매수 텍스트+
  // 빨간색이 쓰이면 "매수=보유중"으로 오인해 청산 건이 보유 포지션에 없는 게
  // 버그처럼 보인다 — 색을 분리해 진입/청산을 한눈에 구분되게 한다.
  function evTagClass(ev: TradeEvent): string {
    if (evIsClose(ev)) return ev.pnl >= 0 ? 'exit-pos' : 'exit-neg';
    return ev.type === 'buy' ? 'buy-tag' : 'sell-tag';
  }
  function fmtPnl(v: number) {
    return (v >= 0 ? '+' : '') + Math.round(v).toLocaleString() + '원';
  }
  function fmtPct(v: number) {
    return (v >= 0 ? '+' : '') + v.toFixed(2) + '%';
  }
  function orderTypeLabel(t: string) {
    return t === 'market' ? '시장가' : t === 'best' ? '최유리지정가' : '지정가';
  }
  const onoff = (b: any) => (b ? 'ON' : 'OFF');
  // 모니터링 상태(phase) → 색상 클래스
  function phaseClass(phase: string): string {
    if (phase === '진입') return 'ph-enter';
    if (phase === '보유중') return 'ph-hold';
    if (phase === '쿨다운' || phase === '재매수가드' || phase === '마감임박' || phase === '마감청산') return 'ph-cool';
    if (phase === '확인봉대기' || phase === '게이트미달' || phase === '상위TF역행' || phase === '숏차단') return 'ph-gate';
    if (phase === '진입제한' || phase === '주문실패' || phase === '수량부족') return 'ph-block';
    return 'ph-idle';
  }
  $: monitorRows = (status.monitor ?? []) as any[];
  // 퍼널 진단: 종목들이 지금 어느 단계(phase)에 몇 개씩 걸려 있는지 집계 — 많은 순 정렬.
  $: phaseSummary = Object.entries((status.monitor_summary ?? {}) as Record<string, number>)
    .sort((a, b) => b[1] - a[1]);
  const asPct = (v: number) => (v * 100).toFixed(v * 100 % 1 === 0 ? 0 : 1) + '%';
  // 실행 중인 엔진 설정 — status가 우선, 없으면 폼의 현재 입력값으로 폴백
  $: cfgRisk = status.risk ?? null;
  $: cfgOrder = status.order ?? null;
  $: symStratEntries = Object.entries((status.symbol_strategies ?? {}) as Record<string, { strategy: string; tf: string }>);
  // 모니터링 종목에 실제 적용되는 전략: 종목별 배정이 있으면 그것을, 없으면 전역 전략을 쓴다.
  function monitorStrategy(code: string): { strategy: string; tf: string; assigned: boolean } {
    const sym = (status.symbol_strategies ?? {}) as Record<string, { strategy: string; tf: string }>;
    const s = sym[code];
    if (s) return { strategy: s.strategy, tf: s.tf, assigned: true };
    // 백엔드 status 는 'timeframe' 키로 내려준다 ('tf' 아님).
    return { strategy: status.strategy ?? strategy, tf: status.timeframe ?? tf, assigned: false };
  }
  $: cfgWeights = (status.weights ?? weights) as Record<string, number>;
  $: activeWeights = sources
    .map((s) => [s, cfgWeights[s] ?? 0] as [string, number])
    .filter(([, w]) => w > 0);
  function fmtTime(iso: string) {
    if (!iso) return '-';
    const d = new Date(iso);
    return isNaN(d.getTime()) ? '-' : d.toLocaleTimeString('ko-KR');
  }
</script>

<h1>
  자동매매 컨트롤
  <span class="run-pill {status.running ? (status.mode === 'live' ? 'live' : 'paper') : 'off'}">
    <span class="dot"></span>
    {status.running ? `${status.mode === 'live' ? '실전' : '모의'} 실행중 · 사이클 ${status.cycles ?? 0}회` : '정지 상태'}
  </span>
</h1>
<p class="warn">
  ℹ️ <b>모의투자</b>와 <b>실전투자</b>의 차이는 <b>주문 체결 방식뿐</b>입니다 — 시그널 감지·리스크 관리·포지션 추적은 동일.
  <br>• <b>모의투자</b>: 시그널 발생 시 <b>현재가로 즉시 체결(시뮬레이션)</b>
  • <b>실전투자</b>: 시그널 발생 시 <b>eBest API로 실제 주문 전송</b>
</p>

{#if error}<div class="error">{error}</div>{/if}
{#if info}<div class="info">ℹ️ {info}</div>{/if}

<div class="grid">
  <section class="card">
    <h3>실행 설정</h3>
    <div class="row">
      <label>모드</label>
      <div class="seg">
        <button class:active={mode==='paper'} on:click={() => selectMode('paper')}>모의투자</button>
        <button class:active={mode==='live'} on:click={() => selectMode('live')} class:danger={mode==='live'}
          title={report?.ready ? '검증 완료' : '검증 미달 (참고) — 모드 선택은 자유'}>
          실전투자 {#if report && !report.ready}⚠️{/if}
        </button>
      </div>
    </div>
    <div class="row"><label>전략</label>
      <select bind:value={strategy} on:change={onStrategyChange}>{#each Object.keys(presets) as p}<option>{p}</option>{/each}</select>
    </div>
    <div class="row"><label>타임프레임</label>
      <select bind:value={tf}>{#each ['1m','3m','5m','10m','15m','30m','60m','1d'] as t}<option>{t}</option>{/each}</select>
    </div>
    <div class="row">
      <label>관심종목</label>
      <input bind:value={watchlistText} placeholder="콤마 구분 종목코드" />
      <button class="mini" class:stale={watchlistStale} on:click={loadFromWatchlist}
        title="대시보드에서 선택한 관심종목 불러오기">
        {watchlistStale ? '⚠ 대시보드 선정 반영' : '★ 불러오기'} ({$watchlist.length})
      </button>
    </div>
    {#if watchlistStale}
      <p class="stale-hint">
        ⚠️ 대시보드에서 선정한 관심종목이 위 입력값과 다릅니다. <b>불러오기</b>를 누르지 않으면
        기존 종목으로 시작/재구성됩니다 — 새로 선정한 종목·전략을 적용하려면 먼저 눌러주세요.
      </p>
    {/if}
    <div class="row"><label>폴링(초)</label><input type="number" bind:value={pollSec} min="2" /></div>
    <div class="row">
      <label title="장외/주말에도 엔진을 돌립니다 (테스트용). 실전 모드에서는 스테일 봉 기반 실주문을 막기 위해 사용할 수 없습니다.">장시간 무시</label>
      <input type="checkbox" bind:checked={ignoreHours} disabled={mode === 'live'} />
      {#if mode === 'live'}<span class="unit">실전에서는 사용 불가</span>{/if}
    </div>
    <div class="actions">
      {#if status.running}
        <button class="stop" on:click={stop}>■ 정지</button>
        <button class="reapply" on:click={() => start()}
          title="정지 없이 현재 폼의 관심종목/전략/리스크 설정을 실행 중인 엔진에 적용합니다 (포지션·현금 유지)">
          ⟳ 설정 재적용
        </button>
      {:else}
        <button class="start" on:click={() => start()}>▶ 시작</button>
      {/if}
      <button class="reapply" on:click={sendTelegram} disabled={telegramSending}
        title="현재 상태 · 모니터링 현황 · 최근 40건 거래내역을 stock_monitor 텔레그램 방으로 전송합니다">
        {telegramSending ? '전송 중…' : '✈️ 텔레그램 전송'}
      </button>
      {#if liveAdvisory}<span class="gate-note">⚠️ 검증 미달(참고) — 시작은 가능하나 모의투자 검증을 권장합니다</span>{/if}
    </div>
  </section>

  <section class="card">
    <h3>주문 설정</h3>
    <div class="row">
      <label>주문유형</label>
      <select bind:value={orderType}>
        <option value="limit">지정가</option>
        <option value="market">시장가</option>
        <option value="best">최유리지정가</option>
      </select>
    </div>
    <div class="row">
      <label title="0 = 리스크 기반 자동 산정">1회 수량</label>
      <input type="number" bind:value={fixedQty} min="0" placeholder="0 = 자동" />
      <span class="unit">주</span>
    </div>
    <div class="row">
      <label title="보유 포지션 전체(롱+숏) 진입금액 합계에 대한 한도 — 1회 주문 한도가 아닙니다">
        {mode === 'live' ? '매수 한도액 (실전)' : '매수 한도액 (모의)'}
      </label>
      {#if mode === 'live'}
        <input type="number" bind:value={maxBuyAmountLive} min="0" step="100000" />
      {:else}
        <input type="number" bind:value={maxBuyAmount} min="0" step="100000" />
      {/if}
      <span class="unit">원</span>
      {#if mode === 'live'}
        <button class="mini" type="button" disabled={liveBalanceLoading} on:click={() => ensureLiveBalanceDefault(true)}
          title="계좌 잔고를 다시 조회해 한도액에 채웁니다">
          {liveBalanceLoading ? '조회중…' : '↻ 잔고 불러오기'}
        </button>
      {/if}
    </div>
    <div class="row">
      <label>매도 방식</label>
      <label class="chk"><input type="checkbox" bind:checked={sellAll} /> 전량매도</label>
    </div>
    <div class="row">
      <label title="동시에 보유할 수 있는 최대 종목 수 — 이 수에 도달하면 신규 진입이 '진입제한'으로 차단됩니다">최대 보유종목</label>
      <input type="number" bind:value={maxPositions} min="1" max="50" />
      <span class="unit">종목</span>
    </div>
    <p class="hint">
      1회 수량 0이면 리스크 기준(자본 1%·ATR)으로 자동 산정합니다.
      실전 지정가/최유리 주문은 접수 후 약 15초간 체결을 확인하고, 미체결 잔량은 자동 취소한 뒤
      <b>실제 체결수량·평균체결가만 포지션에 반영</b>합니다.
      매수 한도액은 <b>1회 주문이 아니라 보유 포지션 전체의 진입금액 합계</b>에 대한 한도이며,
      새 진입은 (한도액 − 현재 총 진입금액)과 가용현금 중 작은 값으로 제한됩니다.
      {#if mode === 'live'}실전투자는 기본값으로 계좌 잔고의 50%(현재 {activeMaxBuyAmount.toLocaleString()}원)를 사용합니다 — 필요 시 직접 조정하세요.{/if}
      {#if !sellAll && fixedQty > 0}<br>※ 전량매도 해제 + 1회수량 설정 시, 청산 신호에 {fixedQty}주씩 부분 매도합니다.{/if}
    </p>
  </section>

  <section class="card">
    <h3>손절 · 익절 설정</h3>
    <p class="hint">매수가(평균단가) 대비 고정 비율로 손절/익절선을 잡습니다. <b>0이면 ATR 변동성 기준으로 자동 산정</b>합니다. 물타기로 평단이 바뀌면 새 평단 기준으로 다시 계산됩니다.</p>
    <div class="row">
      <label>손절 비율</label>
      <input type="number" bind:value={stopLossPct} min="0" step="0.1" placeholder="0 = ATR 자동" />
      <span class="unit">% {stopLossPct > 0 ? `(매수가 −${stopLossPct}%에서 손절)` : '(ATR 자동)'}</span>
    </div>
    <div class="row">
      <label>익절 비율</label>
      <input type="number" bind:value={takeProfitPct} min="0" step="0.1" placeholder="0 = ATR 자동" />
      <span class="unit">% {takeProfitPct > 0 ? `(매수가 +${takeProfitPct}%에서 익절)` : '(ATR 자동)'}</span>
    </div>
    {#if stopLossPct > 0 && takeProfitPct > 0}
      <p class="hint">손익비 ≈ <b>{(takeProfitPct / stopLossPct).toFixed(2)} : 1</b> (익절/손절)</p>
    {/if}
  </section>

  <section class="card">
    <h3>자동 매도(청산) 조건</h3>
    <p class="hint">보유 포지션을 자동으로 청산할 트리거를 개별 선택합니다. 각 조건은 <b>닫힌 봉</b>에서 판단하며(실시간 손절은 아래 '진입 품질' 카드에서 별도 설정), 위 손절·익절선을 그대로 사용합니다.</p>
    <div class="row">
      <label title="손절선(매수가 −손절%, 또는 ATR 기준)에 도달하면 청산">손절 청산</label>
      <label class="chk"><input type="checkbox" bind:checked={useStopLoss} /> 손절선 도달 시 자동 매도</label>
    </div>
    <div class="row">
      <label title="익절선(매수가 +익절%, 또는 ATR 기준)에 도달하면 청산">익절 청산</label>
      <label class="chk"><input type="checkbox" bind:checked={useTakeProfit} /> 익절선 도달 시 자동 매도</label>
    </div>
    <div class="row">
      <label title="수익 구간에서 고점 대비 ATR×배수만큼 되돌리면 청산(이익 보호)">트레일링 스탑</label>
      <label class="chk"><input type="checkbox" bind:checked={useTrailingStop} /> 고점 되돌림 시 자동 매도</label>
    </div>
    <div class="row">
      <label title="트레일링 스탑 폭 = ATR × 이 배수. 작을수록 민감(빨리 청산), 클수록 느슨">트레일링 폭</label>
      <input type="number" bind:value={trailingStopAtr} min="0.5" step="0.1" disabled={!useTrailingStop} />
      <span class="unit">× ATR (고점 −ATR×{trailingStopAtr} 하락 시 청산)</span>
    </div>
    <div class="row">
      <label title="15:05부터 신규 진입 중단, 15:10부터 보유 전량 강제 청산 — 오버나이트 갭 리스크를 없앱니다 (장시간 무시 시에는 동작하지 않음)">거래종료 임박 청산</label>
      <label class="chk"><input type="checkbox" bind:checked={eodFlatten} /> 장 마감 전 전량 청산 (15:05 진입중단 · 15:10 청산)</label>
    </div>
    {#if !useStopLoss}
      <p class="hint warn-hint">⚠️ 손절 청산을 끄면 가격 기반 손절이 완전히 비활성화됩니다(실시간 손절 포함). {eodFlatten ? '거래종료 임박 청산·' : ''}일일 손실 한도만 남습니다 — 위험을 감안하세요.</p>
    {/if}
  </section>

  <section class="card">
    <h3>재진입 가드 (whipsaw 방지)</h3>
    <p class="hint">저가 매도 후 즉시 고가 재매수를 막습니다. 진입·일반청산은 <b>닫힌 봉</b>에서만 판단하고(하드손절만 실시간), 청산 후 일정 봉수 동안 재진입을 막습니다.</p>
    <div class="row">
      <label>손절 후 쿨다운</label>
      <input type="number" bind:value={lossCooldownBars} min="0" /> <span class="unit">봉</span>
    </div>
    <div class="row">
      <label>익절 후 쿨다운</label>
      <input type="number" bind:value={reentryCooldownBars} min="0" /> <span class="unit">봉</span>
    </div>
    <div class="row">
      <label>재매수 가격가드</label>
      <input type="number" bind:value={reentryGapPct} min="0" step="0.1" /> <span class="unit">% (손절가보다 이만큼 낮을 때만 재매수)</span>
    </div>
    <div class="row">
      <label title="손절 후 이 봉수가 지나면 가격가드를 자동 해제합니다 — 회복·상승 전환된 종목의 재진입이 영구히 막히지 않도록">가드 자동해제</label>
      <input type="number" bind:value={reentryGuardExpireBars} min="0" /> <span class="unit">봉 (0 = 무기한 유지)</span>
    </div>
  </section>

  <section class="card">
    <h3>진입 품질 · 청산 안정화</h3>
    <p class="hint">떨어지는 칼날(하락 중 반전 패턴 추격)을 거르고, 틱 노이즈로 인한 즉시 손절을 줄입니다.</p>
    <div class="row">
      <label>OOS 선별</label>
      <label class="chk"><input type="checkbox" bind:checked={requireTradeable} /> 검증 통과 종목만 매매</label>
    </div>
    <div class="row">
      <label>확인봉</label>
      <label class="chk"><input type="checkbox" bind:checked={requireConfirmation} /> 확인봉(양봉/고점돌파) 시에만 진입</label>
    </div>
    <div class="row">
      <label title="패턴 발생 후 이 봉수 안에 확인봉이 나오면 진입 — 1이면 예전처럼 '바로 다음 봉'만 인정">확인 유효기간</label>
      <input type="number" bind:value={confirmWindowBars} min="1" max="10" disabled={!requireConfirmation} />
      <span class="unit">봉 (패턴 후 이 안에 확인되면 진입)</span>
    </div>
    <div class="row">
      <label>상위TF 추세</label>
      <label class="chk"><input type="checkbox" bind:checked={requireHigherTfUptrend} /> 상위 TF 하락이면 진입 금지</label>
    </div>
    <div class="row">
      <label title="0이면 상위 TF 기울기가 조금만 음수여도 차단(엄격). 값을 주면 봉당 이 %까지의 완만한 하락은 허용하고 뚜렷한 하락만 차단합니다">허용 기울기</label>
      <input type="number" bind:value={higherTfTolerancePct} min="0" step="0.01" disabled={!requireHigherTfUptrend} />
      <span class="unit">%/봉 (이 이하의 완만한 하락은 허용, 0=엄격)</span>
    </div>
    <div class="row">
      <label>최소 보유</label>
      <input type="number" bind:value={minHoldBars} min="0" /> <span class="unit">봉 (이 전엔 트레일링만 보류 — 손절·익절은 즉시)</span>
    </div>
    <div class="row">
      <label>실시간 손절</label>
      <label class="chk"><input type="checkbox" bind:checked={hardStopIntrabar} /> 형성 중 봉으로도 손절(off=닫힌 봉만)</label>
    </div>
    <div class="row">
      <label>손절 버퍼</label>
      <input type="number" bind:value={hardStopBufferPct} min="0" step="0.1" disabled={!hardStopIntrabar} />
      <span class="unit">% (실시간 손절 시 손절선보다 이만큼 더 내려야 발동)</span>
    </div>
  </section>

  <section class="card">
    <h3>피보나치 평균매수 (물타기)</h3>
    <p class="hint">손절 신호 시 <b>매도 대신</b> 피보나치 수량(1·1·2·3·5…×최초수량)으로 추가 매수해 평단을 낮춥니다. 설정한 차수를 모두 소진하면 손절합니다. ⚠️ 하락 지속 시 손실이 커질 수 있는 공격적 기법입니다.</p>
    <div class="row">
      <label>물타기 사용</label>
      <label class="chk"><input type="checkbox" bind:checked={fibEnabled} /> 활성화</label>
    </div>
    <div class="row">
      <label>최대 차수</label>
      <input type="number" bind:value={fibMaxLevels} min="1" max="5" disabled={!fibEnabled} />
      <span class="unit">차 (권장 ≤2 · 최대 5 — 3차부터 추가수량 급증)</span>
    </div>
  </section>

  <section class="card">
    <ReadinessPanel {report} />
  </section>

  <section class="card">
    <h3>전략 가중치 (혼합/선택)</h3>
    {#each sources as s}
      <div class="wrow">
        <label>{s}</label>
        <input type="range" min="0" max="1" step="0.05" bind:value={weights[s]} />
        <span>{(weights[s] ?? 0).toFixed(2)}</span>
      </div>
    {/each}
    <div class="wrow">
      <label>진입 임계</label>
      <input type="range" min="0.3" max="0.95" step="0.01" bind:value={entryThreshold} />
      <span>{entryThreshold.toFixed(2)}</span>
    </div>
    {#if status.running}<button class="hot" on:click={pushStrategy}>실행 중 적용</button>{/if}
    <p class="hint">가중치 0 = 비활성. 종합점수는 활성 소스의 가중평균으로 정규화됩니다.</p>
  </section>

  <section class="card status">
    <h3>상태</h3>
    <div class="stat">
      <div><span class="dot" class:on={status.running}></span> {status.running ? '실행중' : '정지'}</div>
      <div>모드: <strong class={status.mode === 'live' ? 'neg' : ''}>{status.mode === 'live' ? '실전투자' : '모의투자'}</strong></div>
      <div>사이클: {status.cycles ?? 0}</div>
      <div>현금: {Math.round(status.cash ?? 0).toLocaleString()}원</div>
      <div>평가자산: <strong>{Math.round(liveEquity).toLocaleString()}원</strong></div>
      <div class={liveUnrealTotal >= 0 ? 'pos' : 'neg'}>미실현: {fmtPnl(liveUnrealTotal)}</div>
      <div class={status.daily_pnl >= 0 ? 'pos' : 'neg'}>일손익: {fmtPnl(status.daily_pnl ?? 0)}</div>
      {#if positions.length && liveAt}<div class="liveat">⟳ 현재가 {liveAt}</div>{/if}
    </div>
    {#if (status.unmanaged_holdings ?? []).length > 0}
      <p class="unmanaged">
        ⚠️ 계좌에 엔진이 관리하지 않는 보유 종목이 있습니다:
        <b>{status.unmanaged_holdings.join(', ')}</b>
        — 자동 손절/익절이 적용되지 않으니 직접 관리하거나 수동 매도하세요.
      </p>
    {/if}
    {#if cfgOrder}
      <div class="cfg-summary">
        <div class="cfg-group">
          <h5>실행</h5>
          <dl>
            <div><dt>전략</dt><dd>{status.strategy ?? strategy}</dd></div>
            <div><dt>방향</dt><dd>
              {status.direction ?? '-'}
              {#if status.direction === 'both' || status.direction === 'short_only'}
                <span class="sub-note">— 숏 신호는 감지만 되고 진입은 항상 차단됩니다 (모의·실전 공통, 공매도 미지원)</span>
              {/if}
            </dd></div>
            <div><dt>타임프레임</dt><dd>{status.timeframe ?? tf}</dd></div>
            <div><dt>진입 임계</dt><dd>{(status.entry_threshold ?? entryThreshold).toFixed(2)}</dd></div>
            <div><dt>폴링 주기</dt><dd>{status.poll_sec ?? pollSec}초</dd></div>
            <div><dt>장시간 무시</dt><dd>{onoff(status.ignore_market_hours ?? ignoreHours)}</dd></div>
            <div><dt>시드 자금</dt><dd>{Math.round(status.seed_cash ?? 0).toLocaleString()}원</dd></div>
            <div class="wide"><dt>관심종목</dt><dd>{(status.watchlist ?? []).join(', ') || '-'}
              {#if extraHeldCodes.length}
                <span class="sub-note">+ 보유 {extraHeldCodes.join(', ')} (워치리스트 외 보유분도 계속 모니터링)</span>
              {/if}
            </dd></div>
            {#if symStratEntries.length}
              <div class="wide"><dt>종목별 전략</dt><dd class="sym-strats">
                {#each symStratEntries as [code, v]}
                  <span class="sym-chip">{code} → <b>{v.strategy}</b> <em>{v.tf}</em></span>
                {/each}
                <span class="sub-note">배정 없는 종목은 전역 전략({status.strategy ?? strategy}) 적용</span>
              </dd></div>
            {/if}
          </dl>
        </div>

        <div class="cfg-group">
          <h5>주문</h5>
          <dl>
            <div><dt>주문유형</dt><dd>{orderTypeLabel(cfgOrder.order_type)}</dd></div>
            <div><dt>1회 수량</dt><dd>{cfgOrder.fixed_qty ? cfgOrder.fixed_qty + '주' : '자동(리스크 기준)'}</dd></div>
            <div><dt>매수 한도액</dt><dd>{Math.round(cfgOrder.max_buy_amount).toLocaleString()}원 (총 진입금액 한도)</dd></div>
            <div><dt>매도 방식</dt><dd>{cfgOrder.sell_all ? '전량매도' : '부분매도'}</dd></div>
          </dl>
        </div>

        {#if cfgRisk}
          <div class="cfg-group">
            <h5>리스크 · 사이징</h5>
            <dl>
              <div><dt>거래당 리스크</dt><dd>{asPct(cfgRisk.risk_per_trade_pct)}</dd></div>
              <div><dt>종목당 비중상한</dt><dd>{asPct(cfgRisk.max_position_pct)}</dd></div>
              <div><dt>최대 보유종목</dt><dd>{cfgRisk.max_positions}개</dd></div>
              <div><dt>일손실 한도</dt><dd>{asPct(cfgRisk.daily_loss_limit_pct)}</dd></div>
              <div><dt>손절 청산</dt><dd class={cfgRisk.use_stop_loss === false ? 'muted' : 'pos'}>
                {cfgRisk.use_stop_loss === false ? 'OFF' : (cfgRisk.stop_loss_pct > 0 ? '−' + asPct(cfgRisk.stop_loss_pct) + ' 고정' : 'ATR ×' + cfgRisk.stop_loss_atr_mult)}</dd></div>
              <div><dt>익절 청산</dt><dd class={cfgRisk.use_take_profit === false ? 'muted' : 'pos'}>
                {cfgRisk.use_take_profit === false ? 'OFF' : (cfgRisk.take_profit_pct > 0 ? '+' + asPct(cfgRisk.take_profit_pct) + ' 고정' : 'ATR ×' + cfgRisk.take_profit_atr_mult)}</dd></div>
              <div><dt>트레일링 청산</dt><dd class={cfgRisk.use_trailing_stop === false ? 'muted' : 'pos'}>
                {cfgRisk.use_trailing_stop === false ? 'OFF' : 'ATR ×' + cfgRisk.trailing_stop_atr}</dd></div>
              <div><dt>거래종료 임박 청산</dt><dd class={cfgRisk.eod_flatten ? 'pos' : 'muted'}>{onoff(cfgRisk.eod_flatten)}</dd></div>
            </dl>
          </div>

          <div class="cfg-group">
            <h5>재진입 가드</h5>
            <dl>
              <div><dt>손절 후 쿨다운</dt><dd>{cfgRisk.loss_cooldown_bars}봉</dd></div>
              <div><dt>익절 후 쿨다운</dt><dd>{cfgRisk.reentry_cooldown_bars}봉</dd></div>
              <div><dt>재매수 가격가드</dt><dd>{asPct(cfgRisk.reentry_gap_pct)}</dd></div>
              <div><dt>가드 자동해제</dt><dd>{cfgRisk.reentry_guard_expire_bars > 0 ? cfgRisk.reentry_guard_expire_bars + '봉' : '무기한'}</dd></div>
            </dl>
          </div>

          <div class="cfg-group">
            <h5>진입 품질 · 청산 안정화</h5>
            <dl>
              <div><dt>확인봉</dt><dd>{onoff(cfgRisk.require_confirmation)}{#if cfgRisk.require_confirmation} · {cfgRisk.confirm_window_bars ?? 1}봉 내{/if}</dd></div>
              <div><dt>상위TF 추세</dt><dd>{onoff(cfgRisk.require_higher_tf_uptrend)}{#if cfgRisk.require_higher_tf_uptrend && (cfgRisk.higher_tf_slope_tolerance ?? 0) > 0} · 허용 {(cfgRisk.higher_tf_slope_tolerance * 100).toFixed(2)}%/봉{/if}</dd></div>
              <div><dt>최소 보유</dt><dd>{cfgRisk.min_hold_bars}봉 (트레일링만)</dd></div>
              <div><dt>실시간 손절</dt><dd>{onoff(cfgRisk.hard_stop_intrabar)}</dd></div>
              <div><dt>손절 버퍼</dt><dd>{asPct(cfgRisk.hard_stop_buffer_pct)}</dd></div>
            </dl>
          </div>

          <div class="cfg-group">
            <h5>피보나치 평균매수</h5>
            <dl>
              <div><dt>물타기</dt><dd class={cfgRisk.fib_averaging_enabled ? 'pos' : ''}>{onoff(cfgRisk.fib_averaging_enabled)}</dd></div>
              <div><dt>최대 차수</dt><dd class={cfgRisk.fib_averaging_enabled ? '' : 'muted'}>{cfgRisk.fib_max_levels}차</dd></div>
            </dl>
          </div>
        {/if}

        <div class="cfg-group">
          <h5>전략 가중치</h5>
          <dl>
            {#each activeWeights as [s, w]}
              <div><dt>{s}</dt><dd>{w.toFixed(2)}</dd></div>
            {/each}
            {#if activeWeights.length === 0}<div class="wide"><dd class="muted">활성 소스 없음</dd></div>{/if}
          </dl>
        </div>
      </div>
    {/if}

    <h4>모니터링 현황 ({monitorRows.length}) <span class="sub-note">— 각 종목에 적용 중인 전략(적용전략)과 매 사이클 판정 결과 (마지막 판정 기준)</span></h4>
    {#if phaseSummary.length > 0}
      <div class="funnel" title="종목들이 지금 어느 판정 단계에 몇 개씩 있는지 — 진입이 적다면 가장 큰 배지의 게이트를 완화하세요">
        {#each phaseSummary as [ph, n]}
          <span class="phase {phaseClass(ph)}">{ph} {n}</span>
        {/each}
      </div>
    {/if}
    <table>
      <thead>
        <tr>
          <th>종목</th><th>코드</th><th>적용전략</th><th>상태</th><th>감지패턴</th><th>점수</th><th>종가</th><th>현재가</th><th>판정 사유</th><th>갱신</th>
        </tr>
      </thead>
      <tbody>
        {#each monitorRows as m}
          {@const ms = monitorStrategy(m.code)}
          <tr>
            <td><strong>{m.name || '-'}</strong></td>
            <td class="cd">{m.code}</td>
            <td class="strat">
              <span class="strat-name" class:global={!ms.assigned}>{ms.strategy}</span>
              <em class="strat-tf">{ms.tf}</em>
              {#if !ms.assigned}<span class="strat-badge" title="종목별 배정 없음 — 전역 전략 적용">전역</span>{/if}
            </td>
            <td><span class="phase {phaseClass(m.phase)}">{m.phase}</span></td>
            <td class="sig">{m.pattern || '-'}</td>
            <td class="num">{m.score != null ? m.score.toFixed(3) : '-'}</td>
            <td class="num">{m.bar_price ? Math.round(m.bar_price).toLocaleString() : '-'}</td>
            <td class="num live">{m.live_price ? Math.round(m.live_price).toLocaleString() : '-'}</td>
            <td class="detail">{m.detail || '-'}</td>
            <td class="cd">{m.at || '-'}</td>
          </tr>
        {/each}
        {#if monitorRows.length === 0}
          <tr><td colspan="10" class="empty">
            {status.running ? '첫 사이클 평가 대기 중…' : '정지 상태 — 시작하면 종목별 모니터링이 표시됩니다'}
          </td></tr>
        {/if}
      </tbody>
    </table>

    <h4>보유 포지션 ({status.positions?.length ?? 0}) <span class="sub-note">— 진입 시그널로 체결된 보유분 (평균 진입단가 기준 분석)</span></h4>
    <table>
      <thead>
        <tr>
          <th>종목</th><th>코드</th><th>구분</th><th>진입신호</th><th>진입시각</th><th>수량</th>
          <th>평균단가</th><th>총 진입금액</th><th>현재가</th><th>총 평가금액</th>
          <th>미실현손익</th><th>수익률</th><th>손절</th><th>익절</th><th>청산</th>
        </tr>
      </thead>
      <tbody>
        {#each positions as p}
          <tr>
            <td><strong>{p.name || '-'}</strong></td>
            <td class="cd">{p.code}</td>
            <td class={p.side === 'short' ? 'sell-tag' : 'buy-tag'}>{p.side === 'short' ? '숏' : '롱'}</td>
            <td class="sig">{p.pattern || '-'}</td>
            <td class="cd">{fmtTime(p.opened_at)}</td>
            <td class="num">{p.qty}</td>
            <td class="num buy">{Math.round(avgPrice(p)).toLocaleString()}</td>
            <td class="num">{Math.round(totalBuy(p)).toLocaleString()}</td>
            <td class="num live">{Math.round(liveCur(p)).toLocaleString()}</td>
            <td class="num">{Math.round(totalEval(p)).toLocaleString()}</td>
            <td class="num {liveUpnl(p) >= 0 ? 'pos' : 'neg'}">{fmtPnl(liveUpnl(p))}</td>
            <td class="num {liveUpct(p) >= 0 ? 'pos' : 'neg'}">{fmtPct(liveUpct(p))}</td>
            <td class="num neg">{Math.round(p.stop).toLocaleString()}</td>
            <td class="num pos">{Math.round(p.target).toLocaleString()}</td>
            <td class="ctd">
              <button class="close-pos" disabled={closing[p.code]} on:click={() => closePosition(p)}
                title={p.side === 'short' ? '이 종목을 전량 청산(매수 환매)합니다' : '이 종목을 전량 청산(매도)합니다'}>
                {closing[p.code] ? '…' : '✕ 청산'}
              </button>
            </td>
          </tr>
        {/each}
        {#if positions.length === 0}<tr><td colspan="15" class="empty">보유 없음 — 진입 시그널 발생 시 자동 진입됩니다</td></tr>{/if}
      </tbody>
      {#if positions.length > 0}
        <tfoot>
          <tr class="totals-row">
            <td colspan="5">합계 ({positions.length}종목)</td>
            <td class="num">{totalQty}</td>
            <td class="num">-</td>
            <td class="num">{Math.round(totalBuyAmt).toLocaleString()}</td>
            <td class="num">-</td>
            <td class="num">{Math.round(totalEvalAmt).toLocaleString()}</td>
            <td class="num {liveUnrealTotal >= 0 ? 'pos' : 'neg'}">{fmtPnl(liveUnrealTotal)}</td>
            <td class="num {totalUpct >= 0 ? 'pos' : 'neg'}">{fmtPct(totalUpct)}</td>
            <td colspan="3"></td>
          </tr>
        </tfoot>
      {/if}
    </table>

    {#if positions.length > 0}
      <div class="limit-compare {buyLimitDiff > 0 ? 'over' : 'under'}">
        <span class="lc-label">총매수금액 합계</span>
        <span class="lc-val">{Math.round(totalBuyAmt).toLocaleString()}원</span>
        <span class="lc-vs">vs</span>
        <span class="lc-label">매수 한도액 (총 진입금액)</span>
        <span class="lc-val">{Math.round(buyLimit).toLocaleString()}원</span>
        <span class="lc-diff">
          차이 <b>{buyLimitDiff >= 0 ? '+' : ''}{Math.round(buyLimitDiff).toLocaleString()}원</b>
          · <b>{buyLimitRatio.toFixed(0)}%</b>{buyLimitRatio > 100 ? ` (한도 ${(buyLimitRatio / 100).toFixed(1)}배)` : ''}
        </span>
      </div>
    {/if}

    {#if tradeEvents.length > 0}
      <div class="h4row">
        <h4>이번 세션 거래 ({tradeEvents.length}건)</h4>
        <button class="clear-btn" on:click={clearSessionEvents}>🗑 세션 거래 지우기</button>
      </div>
      <table>
        <thead><tr><th>시간</th><th>종목</th><th>구분</th><th>수량</th><th>체결가</th><th>실현손익</th><th>수익률</th><th>사유</th></tr></thead>
        <tbody>
          {#each [...tradeEvents].reverse() as ev}
            <tr>
              <td class="cd">{ev.time_label}</td>
              <td><strong>{ev.name || ev.code}</strong><span class="cd"> {ev.code}</span></td>
              <td class={evTagClass(ev)}>{evLabel(ev)}</td>
              <td class="num">{ev.qty.toLocaleString()}</td>
              <td class="num">{Math.round(ev.price).toLocaleString()}</td>
              <td class="num {ev.pnl >= 0 ? 'pos' : 'neg'}">{evIsClose(ev) ? fmtPnl(ev.pnl) : '-'}</td>
              <td class="num {ev.pnl_pct >= 0 ? 'pos' : 'neg'}">{evIsClose(ev) ? fmtPct(ev.pnl_pct) : '-'}</td>
              <td class="cd">{ev.reason}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}

    {#if status.last_error}<p class="err">{status.last_error}</p>{/if}
  </section>
</div>

<!-- 집계 통계 -->
<section class="card stats-card">
  <div class="stats-header">
    <h3>누적 거래 통계</h3>
    <div class="hdr-right">
      <div class="seg sm">
        <button class:active={journalMode==='paper'} on:click={() => journalMode='paper'}>모의</button>
        <button class:active={journalMode==='live'} on:click={() => journalMode='live'}>실전</button>
        <button class:active={journalMode==='all'} on:click={() => journalMode='all'}>전체</button>
      </div>
      {#if tradeStats && tradeStats.count > 0}
        <button class="clear-btn" on:click={clearJournalRecords}>🗑 기록 삭제</button>
      {/if}
    </div>
  </div>
  {#if tradeStats && tradeStats.count > 0}
    <div class="stats-grid">
      <div class="sbox"><div class="slabel">총 거래</div><div class="sval">{tradeStats.count}건</div></div>
      <div class="sbox"><div class="slabel">승/패</div><div class="sval">{tradeStats.win_count}승 {tradeStats.loss_count}패</div></div>
      <div class="sbox"><div class="slabel">승률</div><div class="sval {tradeStats.win_rate >= 50 ? 'pos' : 'neg'}">{tradeStats.win_rate}%</div></div>
      <div class="sbox"><div class="slabel">총 손익</div><div class="sval {tradeStats.total_pnl >= 0 ? 'pos' : 'neg'}">{fmtPnl(tradeStats.total_pnl)}</div></div>
      <div class="sbox"><div class="slabel">평균 수익률</div><div class="sval {tradeStats.avg_return_pct >= 0 ? 'pos' : 'neg'}">{fmtPct(tradeStats.avg_return_pct)}</div></div>
      <div class="sbox"><div class="slabel">손익비</div><div class="sval">{tradeStats.profit_factor != null ? tradeStats.profit_factor.toFixed(2) : '-'}</div></div>
      <div class="sbox"><div class="slabel">최고 거래</div><div class="sval pos">{fmtPnl(tradeStats.best_trade_pnl)}</div></div>
      <div class="sbox"><div class="slabel">최악 거래</div><div class="sval neg">{fmtPnl(tradeStats.worst_trade_pnl)}</div></div>
    </div>
  {:else}
    <p class="empty">거래 내역 없음</p>
  {/if}

  {#if journalTrades.length > 0}
    <h4 style="margin-top:16px">거래 내역 (최근 {journalTrades.length}건)</h4>
    <div class="journal-scroll">
      <table>
        <thead>
          <tr><th>청산시간</th><th>종목</th><th>구분</th><th>패턴</th><th>수량</th><th>진입가</th><th>청산가</th><th>실현손익</th><th>수익률</th><th>사유</th></tr>
        </thead>
        <tbody>
          {#each journalTrades as t}
            <tr>
              <td class="cd">{(t.closed_at || '').slice(0, 16).replace('T', ' ')}</td>
              <td class="cd">{t.code}</td>
              <td class={t.side === 'short' ? 'sell-tag' : t.side === 'long' ? 'buy-tag' : 'cd'}>
                {t.side === 'short' ? '숏' : t.side === 'long' ? '롱' : '-'}</td>
              <td class="cd">{t.pattern || '-'}</td>
              <td class="num">{t.qty}</td>
              <td class="num">{Math.round(t.entry).toLocaleString()}</td>
              <td class="num">{Math.round(t.exit).toLocaleString()}</td>
              <td class="num {t.pnl >= 0 ? 'pos' : 'neg'}">{fmtPnl(t.pnl)}</td>
              <td class="num {t.return_pct >= 0 ? 'pos' : 'neg'}">{fmtPct(t.return_pct)}</td>
              <td class="cd">{t.reason}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<section class="card charts">
  <h3>자동매매 종목 실시간 차트 · 보조지표</h3>
  <p class="chint">자동매매 대상(진행 중이면 엔진 워치리스트 + 보유 포지션, 정지 중이면 설정한 관심종목)의 캔들·MA·볼린저·RSI·MACD를 실시간 표시합니다. 모니터링 현황과 동일한 종목 집합입니다. 매수(▲)/매도(▼) 마커 포함.</p>
  <ActiveTradeCharts codes={activeCodes} {tf} refreshSec={10} {tradeEvents} />
</section>

<style>
  h1 { font-size: 22px; margin: 0 0 6px; display: flex; align-items: center; gap: 12px; }
  .run-pill {
    display: inline-flex; align-items: center; gap: 7px; font-size: 12px; font-weight: 700;
    padding: 4px 12px; border-radius: 12px; background: #313244; color: #bac2de; border: 1px solid #45475a;
  }
  .run-pill .dot { width: 8px; height: 8px; border-radius: 50%; background: #a6adc8; }
  .run-pill.paper { color: #a6e3a1; border-color: #3a4a3a; }
  .run-pill.paper .dot { background: #a6e3a1; box-shadow: 0 0 6px #a6e3a1; }
  .run-pill.live { color: #f38ba8; border-color: #4a2f36; }
  .run-pill.live .dot { background: #f38ba8; box-shadow: 0 0 6px #f38ba8; }
  .warn { background: #313244; color: #f9e2af; padding: 8px 12px; border-radius: 6px; font-size: 12px; margin-bottom: 16px; }
  .grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 16px; }
  .card { background: #181825; border-radius: 10px; padding: 16px; }
  .card.status { grid-column: 1 / -1; }
  h3 { margin: 0 0 14px; font-size: 15px; }
  h4 { margin: 16px 0 8px; font-size: 13px; color: #bac2de; }
  .row { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; }
  .row label { width: 90px; font-size: 13px; color: #bac2de; }
  .row input[type="text"], .row input[type="number"], .row input:not([type]), select { flex: 1; background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 6px 8px; }
  .seg { display: flex; gap: 4px; }
  .seg button { padding: 5px 14px; border: 1px solid #45475a; background: #1e1e2e; color: #cdd6f4; border-radius: 4px; cursor: pointer; }
  .seg.sm button { padding: 3px 10px; font-size: 12px; }
  .seg button.active { background: #89b4fa; color: #1e1e2e; font-weight: 600; }
  .seg button.danger.active { background: #f38ba8; }
  .actions { margin-top: 14px; }
  .start { background: #a6e3a1; color: #1e1e2e; }
  .stop { background: #f38ba8; color: #1e1e2e; }
  .reapply { background: #89b4fa; color: #1e1e2e; margin-left: 8px; }
  .funnel { display: flex; flex-wrap: wrap; gap: 6px; margin: 4px 0 10px; }
  .funnel .phase { font-size: 12px; }
  .start, .stop, .hot { border: none; border-radius: 6px; padding: 9px 20px; font-weight: 700; cursor: pointer; }
  .gate-note { margin-left: 12px; font-size: 12px; color: #f9e2af; }
  .mini { background: #313244; color: #cdd6f4; border: none; border-radius: 4px; padding: 6px 10px; cursor: pointer; font-size: 12px; white-space: nowrap; }
  .mini.stale { background: #f9e2af; color: #1e1e2e; font-weight: 700; }
  .stale-hint { font-size: 12px; color: #f9e2af; background: #2a2717; border: 1px solid #45475a; border-radius: 6px; padding: 6px 10px; margin: -2px 0 10px; }
  .cd { color: #a6adc8; font-size: 12px; }
  .charts { margin-top: 16px; }
  .charts h3 { margin: 0 0 4px; font-size: 15px; }
  .chint { font-size: 12px; color: #a6adc8; margin: 0 0 12px; }
  .hot { background: #cba6f7; color: #1e1e2e; margin-top: 10px; }
  .wrow { display: flex; align-items: center; gap: 10px; margin-bottom: 8px; }
  .wrow label { width: 70px; font-size: 13px; color: #bac2de; }
  .wrow input[type="range"] { flex: 1; }
  .wrow span { width: 36px; text-align: right; font-size: 12px; }
  .hint { font-size: 12px; color: #a6adc8; margin-top: 10px; }
  .warn-hint { color: #f9e2af; }
  .unit { font-size: 12px; color: #bac2de; white-space: nowrap; }
  .chk { display: flex; align-items: center; gap: 6px; width: auto; color: #cdd6f4; font-size: 13px; }
  .chk input { width: auto; }
  /* 설정 요약 */
  .cfg-summary {
    margin-top: 12px; padding-top: 12px; border-top: 1px solid #313244;
    display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 12px;
  }
  .cfg-group {
    background: #1e1e2e; border-radius: 8px; padding: 10px 12px;
    height: 500px; overflow-y: auto;
  }
  .cfg-group h5 { margin: 0 0 8px; font-size: 12px; color: #89b4fa; font-weight: 600; }
  .cfg-group dl { margin: 0; display: flex; flex-direction: column; gap: 4px; }
  .cfg-group dl > div { display: flex; justify-content: space-between; gap: 10px; font-size: 12px; }
  .cfg-group dl > div.wide { flex-direction: column; gap: 2px; }
  .cfg-group dt { color: #a6adc8; white-space: nowrap; }
  .cfg-group dd { margin: 0; color: #cdd6f4; font-weight: 600; text-align: right; font-variant-numeric: tabular-nums; }
  .cfg-group dl > div.wide dd { text-align: left; font-weight: 500; word-break: break-all; }
  .cfg-group dd.muted { color: #a6adc8; font-weight: 400; }
  .stat { display: flex; gap: 24px; flex-wrap: wrap; font-size: 14px; }
  .dot { display: inline-block; width: 9px; height: 9px; border-radius: 50%; background: #a6adc8; }
  .dot.on { background: #a6e3a1; }
  .pos { color: #a6e3a1; } .neg { color: #f38ba8; }
  table { width: 100%; border-collapse: collapse; font-size: 12px; }
  th { text-align: left; padding: 5px 6px; color: #bac2de; border-bottom: 1px solid #313244; font-weight: 500; }
  td { padding: 5px 6px; border-bottom: 1px solid #232334; }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  .num.live { color: #f9e2af; font-weight: 600; }
  .num.buy { color: #cdd6f4; font-weight: 600; }
  .sig { color: #a6e3a1; font-size: 12px; }
  .sub-note { font-size: 12px; color: #a6adc8; font-weight: 400; }
  .sym-strats { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
  .sym-chip { background: #1e1e2e; border: 1px solid #313244; border-radius: 10px; padding: 2px 8px; font-size: 12px; }
  .sym-chip b { color: #cba6f7; }
  .sym-chip em { color: #a6adc8; font-style: normal; font-size: 11px; }
  .strat { white-space: nowrap; }
  .strat-name { color: #cba6f7; font-weight: 600; font-size: 12px; }
  .strat-name.global { color: #bac2de; font-weight: 500; }
  .strat-tf { color: #a6adc8; font-style: normal; font-size: 11px; margin-left: 4px; }
  .strat-badge {
    margin-left: 4px; padding: 1px 5px; border-radius: 8px;
    font-size: 10px; color: #a6adc8; background: #1e1e2e; border: 1px solid #313244;
  }
  .detail { color: #cdd6f4; font-size: 12px; }
  .phase {
    display: inline-block; padding: 2px 8px; border-radius: 10px;
    font-size: 11px; font-weight: 600; white-space: nowrap;
  }
  .ph-enter { background: #2a3a2a; color: #a6e3a1; }
  .ph-hold  { background: #2a3346; color: #89b4fa; }
  .ph-cool  { background: #3a3526; color: #f9e2af; }
  .ph-gate  { background: #3a2f24; color: #fab387; }
  .ph-block { background: #45282f; color: #f38ba8; }
  .ph-idle  { background: #2a2a3a; color: #bac2de; }
  .ctd { text-align: center; }
  .close-pos {
    background: #45282f; color: #f38ba8; border: 1px solid #f38ba8;
    border-radius: 5px; padding: 3px 9px; font-size: 11px; cursor: pointer; white-space: nowrap;
  }
  .close-pos:hover:not(:disabled) { background: #f38ba8; color: #1e1e2e; }
  .close-pos:disabled { opacity: 0.5; cursor: progress; }
  .liveat { font-size: 12px; color: #a6adc8; align-self: center; }
  .totals-row { background: #1e1e2e; font-weight: 700; }
  .totals-row td { border-top: 2px solid #45475a; border-bottom: none; }
  .totals-row td:first-child { color: #bac2de; font-weight: 600; }
  .limit-compare {
    display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
    margin-top: 10px; padding: 10px 14px; border-radius: 8px; font-size: 13px;
    background: #1e1e2e; border: 1px solid #45475a;
  }
  .limit-compare .lc-label { color: #bac2de; }
  .limit-compare .lc-val { font-weight: 700; color: #cdd6f4; }
  .limit-compare .lc-vs { color: #a6adc8; font-size: 11px; }
  .limit-compare .lc-diff { margin-left: auto; font-size: 13px; }
  .limit-compare.over { border-color: #4a2f36; }
  .limit-compare.over .lc-diff { color: #f38ba8; }
  .limit-compare.under .lc-diff { color: #a6e3a1; }
  .h4row { display: flex; justify-content: space-between; align-items: center; }
  .h4row h4 { margin: 16px 0 8px; }
  .hdr-right { display: flex; align-items: center; gap: 10px; }
  .clear-btn {
    background: #313244; color: #f38ba8; border: 1px solid #45475a;
    border-radius: 5px; padding: 4px 10px; font-size: 12px; cursor: pointer;
  }
  .clear-btn:hover { background: #45475a; }
  .empty { color: #a6adc8; text-align: center; }
  .error, .err { background: #f38ba8; color: #1e1e2e; padding: 8px; border-radius: 6px; font-size: 12px; margin-top: 8px; }
  .info { background: #313244; color: #89b4fa; border: 1px solid #45475a; padding: 8px 12px; border-radius: 6px; font-size: 12px; margin-top: 8px; }
  .unmanaged { background: #45475a; color: #f9e2af; padding: 8px 12px; border-radius: 6px; font-size: 12px; margin: 8px 0 0; }
  .buy-tag { color: #ef4444; font-weight: 700; }
  .sell-tag { color: #3b82f6; font-weight: 700; }
  /* 청산(close) 이벤트는 진입(open)과 다른 색 계열(초록/분홍)로 — 매수/매도의
     빨강/파랑과 겹치지 않게 해 "청산도 보유 중"이라는 오독을 막는다. */
  .exit-pos { color: #a6e3a1; font-weight: 700; }
  .exit-neg { color: #f38ba8; font-weight: 700; }
  /* 집계 통계 */
  .stats-card { margin-top: 16px; }
  .stats-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 14px; }
  .stats-header h3 { margin: 0; }
  .stats-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 10px; }
  .sbox { background: #1e1e2e; border-radius: 8px; padding: 10px 14px; }
  .slabel { font-size: 12px; color: #a6adc8; margin-bottom: 4px; }
  .sval { font-size: 16px; font-weight: 700; font-variant-numeric: tabular-nums; }
  .journal-scroll { max-height: 280px; overflow-y: auto; }
</style>
