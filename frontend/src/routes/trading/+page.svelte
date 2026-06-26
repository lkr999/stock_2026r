<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { api, type ReadinessReport, type Timeframe, type TradeEvent, type TradeStats } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';
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
  let ignoreHours = saved.ignoreHours ?? true;

  // 주문 설정
  let orderType = saved.orderType ?? 'limit';          // limit=지정가 | market=시장가 | best=최유리지정가
  let fixedQty = saved.fixedQty ?? 0;                  // 1회 매수/매도 수량 (0=자동 산정)
  let sellAll = saved.sellAll ?? true;                 // 매도 시 전량
  let maxBuyAmount = saved.maxBuyAmount ?? 500000;     // 1회 매수 한도액

  // 손절·익절 설정 (수동 고정%; 0 = ATR 변동성 기준 자동)
  let stopLossPct = saved.stopLossPct ?? 0;            // 매수가 대비 손절 % (예: 3 → -3%)
  let takeProfitPct = saved.takeProfitPct ?? 0;        // 매수가 대비 익절 % (예: 6 → +6%)

  // 재진입 가드 (whipsaw 방지) + 피보나치 평균매수
  let lossCooldownBars = saved.lossCooldownBars ?? 3;       // 손절 청산 후 진입 금지 봉수
  let reentryCooldownBars = saved.reentryCooldownBars ?? 1; // 익절 청산 후 진입 금지 봉수
  let reentryGapPct = saved.reentryGapPct ?? 0;             // 손절가 × (1-이값) 이하에서만 재매수 (%)
  let fibEnabled = saved.fibEnabled ?? false;              // 피보나치 평균매수(물타기) 사용
  let fibMaxLevels = saved.fibMaxLevels ?? 3;              // 물타기 최대 차수

  // 진입 품질 게이트 + 청산 안정화 + OOS 선별
  let requireConfirmation = saved.requireConfirmation ?? true;        // 확인봉(다음 봉 양봉/고점돌파)에서만 진입
  let requireHigherTfUptrend = saved.requireHigherTfUptrend ?? true;  // 상위 TF 하락 시 롱 진입 금지
  let minHoldBars = saved.minHoldBars ?? 1;                  // 익절/트레일링 최소 보유봉수
  let hardStopIntrabar = saved.hardStopIntrabar ?? false;    // 형성 중 봉 실시간가로 손절(off=닫힌 봉)
  let hardStopBufferPct = saved.hardStopBufferPct ?? 0;      // intrabar 손절 버퍼(%)
  let requireTradeable = saved.requireTradeable ?? true;     // OOS 검증 통과 종목만 매매

  function loadFromWatchlist() {
    const codes = get(watchlist);
    if (codes.length) watchlistText = codes.join(', ');
  }

  let status: any = { running: false, positions: [], daily_pnl: 0, trade_events: [] };
  let presets: Record<string, any> = {};
  // 가중치/임계는 프리셋에서 채워지지만, 사용자가 조정한 값이 저장돼 있으면 그대로 복원한다.
  let weights: Record<string, number> = saved.weights ? { ...saved.weights } : {};
  let entryThreshold = saved.entryThreshold ?? 0.65;
  const hasSavedWeights = !!saved.weights;
  let error = '';
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
    orderType, fixedQty, sellAll, maxBuyAmount,
    stopLossPct, takeProfitPct,
    lossCooldownBars, reentryCooldownBars, reentryGapPct, fibEnabled, fibMaxLevels,
    requireConfirmation, requireHigherTfUptrend, minHoldBars, hardStopIntrabar, hardStopBufferPct,
    requireTradeable, weights, entryThreshold,
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

  // 포지션별 실시간 현재가/미실현손익 — livePrices 우선, 없으면 엔진 값
  function liveCur(p: any): number {
    return livePrices[p.code] ?? p.current_price ?? p.entry;
  }
  function liveUpnl(p: any): number {
    return (liveCur(p) - p.entry) * p.qty;
  }
  function liveUpct(p: any): number {
    return p.entry ? (liveCur(p) - p.entry) / p.entry * 100 : 0;
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
  // 실시간 평가자산/미실현 합계
  $: positions = status.positions ?? [];
  $: liveEquity = (status.cash ?? 0) + positions.reduce((s: number, p: any) => s + p.qty * liveCur(p), 0);
  $: liveUnrealTotal = positions.reduce((s: number, p: any) => s + liveUpnl(p), 0);

  let closing: Record<string, boolean> = {};
  async function closePosition(p: any) {
    const live = status.mode === 'live';
    const msg = live
      ? `[실전] ${p.name || p.code} ${p.qty}주를 실제 시장가/지정가로 청산 주문합니다. 계속할까요?`
      : `${p.name || p.code} ${p.qty}주를 현재가로 청산(매도)합니다. 계속할까요?`;
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
    : watchlistText.split(',').map((s: string) => s.trim()).filter(Boolean);

  $: tradeEvents = (status.trade_events ?? []) as TradeEvent[];

  // 워치리스트에는 없지만 보유 중이라 계속 모니터링되는 종목(설정 표시용).
  $: extraHeldCodes = ((status.positions ?? []) as any[])
    .map((p) => p.code as string)
    .filter((c) => !((status.watchlist ?? []) as string[]).includes(c));

  async function start() {
    error = '';
    if (mode === 'live' && !report?.ready) {
      const ok = confirm('검증 기준 미달 상태입니다. 그래도 실전투자를 시작하시겠습니까?\n(권장: 모의투자로 충분히 검증 후 진행)');
      if (!ok) return;
    }
    try {
      const wl = watchlistText.split(',').map((s: string) => s.trim()).filter(Boolean);
      const resStart = await api.tradingStart({
        mode, strategy: { name: strategy, weights, entry_threshold: entryThreshold },
        watchlist: wl, tf, poll_sec: pollSec, ignore_market_hours: ignoreHours,
        order: {
          order_type: orderType,
          fixed_qty: fixedQty,
          sell_all: sellAll,
          max_buy_amount: maxBuyAmount,
        },
        require_tradeable: requireTradeable,
        risk: {
          stop_loss_pct: stopLossPct / 100,
          take_profit_pct: takeProfitPct / 100,
          loss_cooldown_bars: lossCooldownBars,
          reentry_cooldown_bars: reentryCooldownBars,
          reentry_gap_pct: reentryGapPct / 100,
          fib_averaging_enabled: fibEnabled,
          fib_max_levels: fibMaxLevels,
          require_confirmation: requireConfirmation,
          require_higher_tf_uptrend: requireHigherTfUptrend,
          min_hold_bars: minHoldBars,
          hard_stop_intrabar: hardStopIntrabar,
          hard_stop_buffer_pct: hardStopBufferPct / 100,
        },
      });
      if (resStart?.dropped_untradeable?.length) {
        error = `OOS 미통과로 제외된 종목: ${resStart.dropped_untradeable.join(', ')}`;
      }
      await refreshStatus();
      await refreshReadiness();
    } catch (e) {
      error = String(e);
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
    timer = setInterval(() => { refreshStatus(); refreshReadiness(); }, 5000);
  });
  onDestroy(() => clearInterval(timer));

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
    if (phase === '쿨다운' || phase === '재매수가드') return 'ph-cool';
    if (phase === '확인봉대기' || phase === '게이트미달' || phase === '상위TF하락') return 'ph-gate';
    if (phase === '진입제한' || phase === '주문실패' || phase === '수량부족') return 'ph-block';
    return 'ph-idle';
  }
  $: monitorRows = (status.monitor ?? []) as any[];
  const asPct = (v: number) => (v * 100).toFixed(v * 100 % 1 === 0 ? 0 : 1) + '%';
  // 실행 중인 엔진 설정 — status가 우선, 없으면 폼의 현재 입력값으로 폴백
  $: cfgRisk = status.risk ?? null;
  $: cfgOrder = status.order ?? null;
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

<h1>자동매매 컨트롤</h1>
<p class="warn">
  ℹ️ <b>모의투자</b>와 <b>실전투자</b>의 차이는 <b>주문 체결 방식뿐</b>입니다 — 시그널 감지·리스크 관리·포지션 추적은 동일.
  <br>• <b>모의투자</b>: 시그널 발생 시 <b>현재가로 즉시 체결(시뮬레이션)</b>
  • <b>실전투자</b>: 시그널 발생 시 <b>eBest API로 실제 주문 전송</b>
</p>

{#if error}<div class="error">{error}</div>{/if}

<div class="grid">
  <section class="card">
    <h3>실행 설정</h3>
    <div class="row">
      <label>모드</label>
      <div class="seg">
        <button class:active={mode==='paper'} on:click={() => mode='paper'}>모의투자</button>
        <button class:active={mode==='live'} on:click={() => mode='live'} class:danger={mode==='live'}
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
      <button class="mini" on:click={loadFromWatchlist} title="대시보드에서 선택한 관심종목 불러오기">★ 불러오기 ({$watchlist.length})</button>
    </div>
    <div class="row"><label>폴링(초)</label><input type="number" bind:value={pollSec} min="2" /></div>
    <div class="row"><label>장시간 무시</label><input type="checkbox" bind:checked={ignoreHours} /></div>
    <div class="actions">
      {#if status.running}
        <button class="stop" on:click={stop}>■ 정지</button>
      {:else}
        <button class="start" on:click={start}>▶ 시작</button>
      {/if}
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
      <label>매수 한도액</label>
      <input type="number" bind:value={maxBuyAmount} min="0" step="100000" />
      <span class="unit">원/회</span>
    </div>
    <div class="row">
      <label>매도 방식</label>
      <label class="chk"><input type="checkbox" bind:checked={sellAll} /> 전량매도</label>
    </div>
    <p class="hint">
      1회 수량 0이면 리스크 기준(자본 1%·ATR)으로 자동 산정합니다.
      매수는 한도액({maxBuyAmount.toLocaleString()}원)과 가용현금 중 작은 값으로 제한됩니다.
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
      <label class="chk"><input type="checkbox" bind:checked={requireConfirmation} /> 다음 봉 확인(양봉/고점돌파) 시에만 진입</label>
    </div>
    <div class="row">
      <label>상위TF 추세</label>
      <label class="chk"><input type="checkbox" bind:checked={requireHigherTfUptrend} /> 상위 TF 하락이면 진입 금지</label>
    </div>
    <div class="row">
      <label>최소 보유</label>
      <input type="number" bind:value={minHoldBars} min="0" /> <span class="unit">봉 (이 전엔 익절/트레일링 보류)</span>
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
      <input type="number" bind:value={fibMaxLevels} min="1" max="10" disabled={!fibEnabled} />
      <span class="unit">차 (예: 3 → 최대 3회 추가매수)</span>
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
    {#if cfgOrder}
      <div class="cfg-summary">
        <div class="cfg-group">
          <h5>실행</h5>
          <dl>
            <div><dt>전략</dt><dd>{status.strategy ?? strategy}</dd></div>
            <div><dt>방향</dt><dd>{status.direction ?? '-'}</dd></div>
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
          </dl>
        </div>

        <div class="cfg-group">
          <h5>주문</h5>
          <dl>
            <div><dt>주문유형</dt><dd>{orderTypeLabel(cfgOrder.order_type)}</dd></div>
            <div><dt>1회 수량</dt><dd>{cfgOrder.fixed_qty ? cfgOrder.fixed_qty + '주' : '자동(리스크 기준)'}</dd></div>
            <div><dt>매수 한도액</dt><dd>{Math.round(cfgOrder.max_buy_amount).toLocaleString()}원/회</dd></div>
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
              <div><dt>손절선</dt><dd>{cfgRisk.stop_loss_pct > 0 ? '−' + asPct(cfgRisk.stop_loss_pct) + ' 고정' : 'ATR ×' + cfgRisk.stop_loss_atr_mult}</dd></div>
              <div><dt>익절선</dt><dd>{cfgRisk.take_profit_pct > 0 ? '+' + asPct(cfgRisk.take_profit_pct) + ' 고정' : 'ATR ×' + cfgRisk.take_profit_atr_mult}</dd></div>
              <div><dt>트레일링 ATR</dt><dd>×{cfgRisk.trailing_stop_atr}</dd></div>
            </dl>
          </div>

          <div class="cfg-group">
            <h5>재진입 가드</h5>
            <dl>
              <div><dt>손절 후 쿨다운</dt><dd>{cfgRisk.loss_cooldown_bars}봉</dd></div>
              <div><dt>익절 후 쿨다운</dt><dd>{cfgRisk.reentry_cooldown_bars}봉</dd></div>
              <div><dt>재매수 가격가드</dt><dd>{asPct(cfgRisk.reentry_gap_pct)}</dd></div>
            </dl>
          </div>

          <div class="cfg-group">
            <h5>진입 품질 · 청산 안정화</h5>
            <dl>
              <div><dt>확인봉</dt><dd>{onoff(cfgRisk.require_confirmation)}</dd></div>
              <div><dt>상위TF 추세</dt><dd>{onoff(cfgRisk.require_higher_tf_uptrend)}</dd></div>
              <div><dt>최소 보유</dt><dd>{cfgRisk.min_hold_bars}봉</dd></div>
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

    <h4>모니터링 현황 ({monitorRows.length}) <span class="sub-note">— 각 종목을 매 사이클 어떻게 평가하고 있는지 (마지막 판정 기준)</span></h4>
    <table>
      <thead>
        <tr>
          <th>종목</th><th>코드</th><th>상태</th><th>감지패턴</th><th>점수</th><th>종가</th><th>현재가</th><th>판정 사유</th><th>갱신</th>
        </tr>
      </thead>
      <tbody>
        {#each monitorRows as m}
          <tr>
            <td><strong>{m.name || '-'}</strong></td>
            <td class="cd">{m.code}</td>
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
          <tr><td colspan="9" class="empty">
            {status.running ? '첫 사이클 평가 대기 중…' : '정지 상태 — 시작하면 종목별 모니터링이 표시됩니다'}
          </td></tr>
        {/if}
      </tbody>
    </table>

    <h4>보유 포지션 ({status.positions?.length ?? 0}) <span class="sub-note">— 실제 매수 시그널로 체결된 보유분 (평균매수단가 기준 분석)</span></h4>
    <table>
      <thead>
        <tr>
          <th>종목</th><th>코드</th><th>매수신호</th><th>진입시각</th><th>수량</th>
          <th>평균단가</th><th>총 매수금액</th><th>현재가</th><th>총 평가금액</th>
          <th>미실현손익</th><th>수익률</th><th>손절</th><th>익절</th><th>청산</th>
        </tr>
      </thead>
      <tbody>
        {#each positions as p}
          <tr>
            <td><strong>{p.name || '-'}</strong></td>
            <td class="cd">{p.code}</td>
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
                title="이 종목을 전량 청산(매도)합니다">
                {closing[p.code] ? '…' : '✕ 청산'}
              </button>
            </td>
          </tr>
        {/each}
        {#if positions.length === 0}<tr><td colspan="14" class="empty">보유 없음 — 매수 시그널 발생 시 자동 진입됩니다</td></tr>{/if}
      </tbody>
    </table>

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
              <td class={ev.type === 'buy' ? 'buy-tag' : 'sell-tag'}>{ev.type === 'buy' ? '매수' : '매도'}</td>
              <td class="num">{ev.qty.toLocaleString()}</td>
              <td class="num">{Math.round(ev.price).toLocaleString()}</td>
              <td class="num {ev.pnl >= 0 ? 'pos' : 'neg'}">{ev.type === 'sell' ? fmtPnl(ev.pnl) : '-'}</td>
              <td class="num {ev.pnl_pct >= 0 ? 'pos' : 'neg'}">{ev.type === 'sell' ? fmtPct(ev.pnl_pct) : '-'}</td>
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
          <tr><th>청산시간</th><th>종목</th><th>패턴</th><th>수량</th><th>진입가</th><th>청산가</th><th>실현손익</th><th>수익률</th><th>사유</th></tr>
        </thead>
        <tbody>
          {#each journalTrades as t}
            <tr>
              <td class="cd">{(t.closed_at || '').slice(0, 16).replace('T', ' ')}</td>
              <td class="cd">{t.code}</td>
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
  h1 { font-size: 22px; margin: 0 0 6px; }
  .warn { background: #313244; color: #f9e2af; padding: 8px 12px; border-radius: 6px; font-size: 12px; margin-bottom: 16px; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
  .card { background: #181825; border-radius: 10px; padding: 16px; }
  .card.status { grid-column: 1 / -1; }
  h3 { margin: 0 0 14px; font-size: 15px; }
  h4 { margin: 16px 0 8px; font-size: 13px; color: #9399b2; }
  .row { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; }
  .row label { width: 90px; font-size: 13px; color: #9399b2; }
  .row input[type="text"], .row input[type="number"], .row input:not([type]), select { flex: 1; background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 6px 8px; }
  .seg { display: flex; gap: 4px; }
  .seg button { padding: 5px 14px; border: 1px solid #45475a; background: #1e1e2e; color: #cdd6f4; border-radius: 4px; cursor: pointer; }
  .seg.sm button { padding: 3px 10px; font-size: 12px; }
  .seg button.active { background: #89b4fa; color: #1e1e2e; font-weight: 600; }
  .seg button.danger.active { background: #f38ba8; }
  .actions { margin-top: 14px; }
  .start { background: #a6e3a1; color: #1e1e2e; }
  .stop { background: #f38ba8; color: #1e1e2e; }
  .start, .stop, .hot { border: none; border-radius: 6px; padding: 9px 20px; font-weight: 700; cursor: pointer; }
  .gate-note { margin-left: 12px; font-size: 12px; color: #f9e2af; }
  .mini { background: #313244; color: #cdd6f4; border: none; border-radius: 4px; padding: 6px 10px; cursor: pointer; font-size: 12px; white-space: nowrap; }
  .cd { color: #6c7086; font-size: 11px; }
  .charts { margin-top: 16px; }
  .charts h3 { margin: 0 0 4px; font-size: 15px; }
  .chint { font-size: 12px; color: #6c7086; margin: 0 0 12px; }
  .hot { background: #cba6f7; color: #1e1e2e; margin-top: 10px; }
  .wrow { display: flex; align-items: center; gap: 10px; margin-bottom: 8px; }
  .wrow label { width: 70px; font-size: 13px; color: #bac2de; }
  .wrow input[type="range"] { flex: 1; }
  .wrow span { width: 36px; text-align: right; font-size: 12px; }
  .hint { font-size: 11px; color: #6c7086; margin-top: 10px; }
  .unit { font-size: 12px; color: #9399b2; white-space: nowrap; }
  .chk { display: flex; align-items: center; gap: 6px; width: auto; color: #cdd6f4; font-size: 13px; }
  .chk input { width: auto; }
  /* 설정 요약 */
  .cfg-summary {
    margin-top: 12px; padding-top: 12px; border-top: 1px solid #313244;
    display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 12px;
  }
  .cfg-group { background: #1e1e2e; border-radius: 8px; padding: 10px 12px; }
  .cfg-group h5 { margin: 0 0 8px; font-size: 12px; color: #89b4fa; font-weight: 600; }
  .cfg-group dl { margin: 0; display: flex; flex-direction: column; gap: 4px; }
  .cfg-group dl > div { display: flex; justify-content: space-between; gap: 10px; font-size: 12px; }
  .cfg-group dl > div.wide { flex-direction: column; gap: 2px; }
  .cfg-group dt { color: #6c7086; white-space: nowrap; }
  .cfg-group dd { margin: 0; color: #cdd6f4; font-weight: 600; text-align: right; font-variant-numeric: tabular-nums; }
  .cfg-group dl > div.wide dd { text-align: left; font-weight: 500; word-break: break-all; }
  .cfg-group dd.muted { color: #6c7086; font-weight: 400; }
  .stat { display: flex; gap: 24px; flex-wrap: wrap; font-size: 14px; }
  .dot { display: inline-block; width: 9px; height: 9px; border-radius: 50%; background: #6c7086; }
  .dot.on { background: #a6e3a1; }
  .pos { color: #a6e3a1; } .neg { color: #f38ba8; }
  table { width: 100%; border-collapse: collapse; font-size: 12px; }
  th { text-align: left; padding: 5px 6px; color: #9399b2; border-bottom: 1px solid #313244; font-weight: 500; }
  td { padding: 5px 6px; border-bottom: 1px solid #232334; }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  .num.live { color: #f9e2af; font-weight: 600; }
  .num.buy { color: #cdd6f4; font-weight: 600; }
  .sig { color: #a6e3a1; font-size: 11px; }
  .sub-note { font-size: 11px; color: #6c7086; font-weight: 400; }
  .detail { color: #bac2de; font-size: 11px; }
  .phase {
    display: inline-block; padding: 2px 8px; border-radius: 10px;
    font-size: 11px; font-weight: 600; white-space: nowrap;
  }
  .ph-enter { background: #2a3a2a; color: #a6e3a1; }
  .ph-hold  { background: #2a3346; color: #89b4fa; }
  .ph-cool  { background: #3a3526; color: #f9e2af; }
  .ph-gate  { background: #3a2f24; color: #fab387; }
  .ph-block { background: #45282f; color: #f38ba8; }
  .ph-idle  { background: #2a2a3a; color: #9399b2; }
  .ctd { text-align: center; }
  .close-pos {
    background: #45282f; color: #f38ba8; border: 1px solid #f38ba8;
    border-radius: 5px; padding: 3px 9px; font-size: 11px; cursor: pointer; white-space: nowrap;
  }
  .close-pos:hover:not(:disabled) { background: #f38ba8; color: #1e1e2e; }
  .close-pos:disabled { opacity: 0.5; cursor: progress; }
  .liveat { font-size: 11px; color: #6c7086; align-self: center; }
  .h4row { display: flex; justify-content: space-between; align-items: center; }
  .h4row h4 { margin: 16px 0 8px; }
  .hdr-right { display: flex; align-items: center; gap: 10px; }
  .clear-btn {
    background: #313244; color: #f38ba8; border: 1px solid #45475a;
    border-radius: 5px; padding: 4px 10px; font-size: 12px; cursor: pointer;
  }
  .clear-btn:hover { background: #45475a; }
  .empty { color: #6c7086; text-align: center; }
  .error, .err { background: #f38ba8; color: #1e1e2e; padding: 8px; border-radius: 6px; font-size: 12px; margin-top: 8px; }
  .buy-tag { color: #ef4444; font-weight: 700; }
  .sell-tag { color: #3b82f6; font-weight: 700; }
  /* 집계 통계 */
  .stats-card { margin-top: 16px; }
  .stats-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 14px; }
  .stats-header h3 { margin: 0; }
  .stats-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 10px; }
  .sbox { background: #1e1e2e; border-radius: 8px; padding: 10px 14px; }
  .slabel { font-size: 11px; color: #6c7086; margin-bottom: 4px; }
  .sval { font-size: 16px; font-weight: 700; font-variant-numeric: tabular-nums; }
  .journal-scroll { max-height: 280px; overflow-y: auto; }
</style>
