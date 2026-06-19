<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { api, type ReadinessReport, type Timeframe, type TradeEvent, type TradeStats } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';
  import ReadinessPanel from '$lib/components/ReadinessPanel.svelte';
  import ActiveTradeCharts from '$lib/components/ActiveTradeCharts.svelte';

  let report: ReadinessReport | null = null;
  let mode = 'paper';
  let strategy = 'balanced';
  let tf: Timeframe = '5m';
  const initialWatch = get(watchlist);
  let watchlistText = initialWatch.length ? initialWatch.join(', ') : '005930, 000660, 035420';
  let pollSec = 60;
  let ignoreHours = true;

  // 주문 설정
  let orderType = 'limit';          // limit=지정가 | market=시장가 | best=최유리지정가
  let fixedQty = 0;                 // 1회 매수/매도 수량 (0=자동 산정)
  let sellAll = true;               // 매도 시 전량
  let maxBuyAmount = 500000;        // 1회 매수 한도액

  function loadFromWatchlist() {
    const codes = get(watchlist);
    if (codes.length) watchlistText = codes.join(', ');
  }

  let status: any = { running: false, positions: [], daily_pnl: 0, trade_events: [] };
  let presets: Record<string, any> = {};
  let weights: Record<string, number> = {};
  let entryThreshold = 0.65;
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
    applyPreset();
  }
  function applyPreset() {
    const p = presets[strategy];
    if (!p) return;
    weights = { ...p.weights };
    entryThreshold = p.entry_threshold;
  }
  $: strategy, applyPreset();

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

  $: activeCodes =
    status.running && status.watchlist?.length
      ? status.watchlist
      : watchlistText.split(',').map((s: string) => s.trim()).filter(Boolean);

  $: tradeEvents = (status.trade_events ?? []) as TradeEvent[];

  async function start() {
    error = '';
    if (mode === 'live' && !report?.ready) {
      const ok = confirm('검증 기준 미달 상태입니다. 그래도 실전투자를 시작하시겠습니까?\n(권장: 모의투자로 충분히 검증 후 진행)');
      if (!ok) return;
    }
    try {
      const wl = watchlistText.split(',').map((s: string) => s.trim()).filter(Boolean);
      await api.tradingStart({
        mode, strategy: { name: strategy, weights, entry_threshold: entryThreshold },
        watchlist: wl, tf, poll_sec: pollSec, ignore_market_hours: ignoreHours,
        order: {
          order_type: orderType,
          fixed_qty: fixedQty,
          sell_all: sellAll,
          max_buy_amount: maxBuyAmount,
        },
      });
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
      <select bind:value={strategy}>{#each Object.keys(presets) as p}<option>{p}</option>{/each}</select>
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
    {#if status.order}
      <div class="orderline">
        주문: <b>{orderTypeLabel(status.order.order_type)}</b> ·
        1회수량 <b>{status.order.fixed_qty ? status.order.fixed_qty + '주' : '자동'}</b> ·
        매수한도 <b>{Math.round(status.order.max_buy_amount).toLocaleString()}원</b> ·
        <b>{status.order.sell_all ? '전량매도' : '부분매도'}</b>
      </div>
    {/if}

    <h4>보유 포지션 ({status.positions?.length ?? 0}) <span class="sub-note">— 실제 매수 시그널로 체결된 보유분 (매수가 기준 분석)</span></h4>
    <table>
      <thead>
        <tr>
          <th>종목</th><th>코드</th><th>매수신호</th><th>진입시각</th><th>수량</th><th>매수가</th><th>현재가</th>
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
            <td class="num buy">{Math.round(p.buy_price ?? p.entry).toLocaleString()}</td>
            <td class="num live">{Math.round(liveCur(p)).toLocaleString()}</td>
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
        {#if positions.length === 0}<tr><td colspan="12" class="empty">보유 없음 — 매수 시그널 발생 시 자동 진입됩니다</td></tr>{/if}
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
  <p class="chint">자동매매 대상(진행 중이면 엔진 워치리스트, 정지 중이면 설정한 관심종목)의 캔들·MA·볼린저·RSI·MACD를 실시간 표시합니다. 매수(▲)/매도(▼) 마커 포함.</p>
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
  .orderline { font-size: 12px; color: #9399b2; margin-top: 10px; padding-top: 8px; border-top: 1px solid #313244; }
  .orderline b { color: #cdd6f4; }
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
