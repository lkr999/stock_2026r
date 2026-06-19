<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, type SignalItem, type EbestStatus, type EbestTestResult } from '$lib/api';
  import { selectedTimeframe, selectedStrategy } from '$lib/stores/timeframe';
  import { watchlist } from '$lib/stores/watchlist';
  import TimeframeSelector from '$lib/components/TimeframeSelector.svelte';
  import SignalTable from '$lib/components/SignalTable.svelte';
  import WatchlistPicker from '$lib/components/WatchlistPicker.svelte';

  // eBest API 통신 상태/테스트
  let ebestStatus: EbestStatus | null = null;
  let ebestResult: EbestTestResult | null = null;
  let testCode = '005930';
  let testing = false;
  let apiPanelOpen = true;

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

  let signals: SignalItem[] = [];
  let loading = false;
  let market = 'ALL';
  let error = '';
  let timer: ReturnType<typeof setInterval>;
  let initialized = false;
  let refreshQueued = false;
  let criteriaKey = '';
  let lastCriteriaKey = '';

  // 현재가 필터 (search_item.csv 의 현재가 기준)
  let minPrice: number | undefined = 1000;
  let maxPrice: number | undefined = 100000;
  let watchlistOnly = false;

  const DASHBOARD_SCAN_LIMIT = 25;
  const strategies = ['conservative', 'balanced', 'aggressive', 'ml_blended'];

  async function refresh() {
    if (loading) {
      refreshQueued = true;
      return;
    }
    loading = true;
    error = '';
    try {
      if (watchlistOnly) {
        const codes = [...$watchlist];
        signals = codes.length
          ? await api.watchlistSignals(codes, $selectedTimeframe, $selectedStrategy, 0.5)
          : [];
      } else {
        signals = await api.signals(
          market,
          $selectedTimeframe,
          $selectedStrategy,
          0.5,
          DASHBOARD_SCAN_LIMIT,
          minPrice,
          maxPrice
        );
      }
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
      if (refreshQueued) {
        refreshQueued = false;
        await refresh();
      }
    }
  }

  $: criteriaKey = JSON.stringify({
    tf: $selectedTimeframe,
    strategy: $selectedStrategy,
    market,
    minPrice,
    maxPrice,
    watchlistOnly,
    codes: watchlistOnly ? $watchlist : []
  });

  $: shown = signals;

  $: if (initialized && criteriaKey !== lastCriteriaKey) {
    lastCriteriaKey = criteriaKey;
    refresh();
  }

  onMount(() => {
    initialized = true;
    lastCriteriaKey = criteriaKey;
    refresh();
    loadEbestStatus();
    timer = setInterval(() => {
      if (!loading) refresh();
    }, 30000);
  });
  onDestroy(() => clearInterval(timer));
</script>

<header>
  <h1>패턴 시그널 대시보드</h1>
  <p class="sub">
    Caginalp & Laurent (1998) 8패턴 + ATR/거래량/MTF 혼합 스코어 · 30초 자동 갱신 ·
    <code>files/search_item.csv</code> 현재가 필터 후 최대 {DASHBOARD_SCAN_LIMIT}종목 스캔
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

<div class="controls">
  <div class="group"><label>타임프레임</label><TimeframeSelector /></div>
  <div class="group">
    <label>전략</label>
    <select bind:value={$selectedStrategy}>{#each strategies as s}<option value={s}>{s}</option>{/each}</select>
  </div>
  <div class="group">
    <label>시장</label>
    <select bind:value={market}><option>ALL</option><option>KOSPI</option><option>KOSDAQ</option></select>
  </div>
  <div class="group">
    <label>현재가 최소</label>
    <input type="number" bind:value={minPrice} step="500" min="0" />
  </div>
  <div class="group">
    <label>현재가 최대</label>
    <input type="number" bind:value={maxPrice} step="500" min="0" />
  </div>
  <button class="refresh" on:click={refresh} disabled={loading}>{loading ? '스캔중…' : '↻ 적용/새로고침'}</button>
  <label class="chk">
    <input type="checkbox" bind:checked={watchlistOnly} /> 관심종목만 ({$watchlist.length})
  </label>
  <a class="to-trading" href="/trading">관심종목으로 자동매매 →</a>
</div>

{#if error}<div class="error">{error}</div>{/if}

<div class="panel">
  <SignalTable signals={shown} {loading} />
</div>

<style>
  header h1 { margin: 0 0 4px; font-size: 22px; }
  .sub { color: #6c7086; margin: 0 0 16px; font-size: 13px; }
  .sub code { background: #313244; padding: 1px 5px; border-radius: 4px; color: #bac2de; }
  .picker-wrap { margin-bottom: 14px; }
  .controls { display: flex; gap: 16px; align-items: flex-end; margin-bottom: 16px; flex-wrap: wrap; }
  .group { display: flex; flex-direction: column; gap: 6px; }
  label { font-size: 12px; color: #9399b2; }
  select, input[type='number'] { background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 5px 8px; width: 110px; }
  .refresh { background: #89b4fa; color: #1e1e2e; border: none; border-radius: 6px; padding: 8px 16px; font-weight: 600; cursor: pointer; }
  .chk { display: flex; align-items: center; gap: 6px; font-size: 13px; color: #cdd6f4; }
  .to-trading { margin-left: auto; color: #a6e3a1; text-decoration: none; font-size: 13px; align-self: center; }
  .panel { background: #181825; border-radius: 10px; padding: 8px 16px; }
  .error { background: #f38ba8; color: #1e1e2e; padding: 10px; border-radius: 6px; margin-bottom: 12px; font-size: 13px; }

  /* eBest API 패널 */
  .api-panel { background: #181825; border: 1px solid #313244; border-radius: 10px; padding: 12px 14px; margin-bottom: 14px; }
  .api-head { display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap; }
  .api-title { display: flex; align-items: center; gap: 8px; font-size: 14px; flex-wrap: wrap; }
  .toggle { background: none; border: none; color: #9399b2; cursor: pointer; font-size: 13px; padding: 0; }
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
  .api-result th { text-align: left; padding: 5px 7px; color: #9399b2; border-bottom: 1px solid #313244; font-weight: 500; }
  .api-result td { padding: 5px 7px; border-bottom: 1px solid #232334; }
  .api-result td.ok { color: #a6e3a1; }
  .api-result td.fail { color: #f38ba8; }
  .api-result td.tr { color: #6c7086; font-family: monospace; }
  .api-result td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .api-result td.detail { color: #bac2de; }
  .api-hint { font-size: 12px; color: #6c7086; margin: 10px 0 0; }
  .api-hint code { background: #313244; padding: 1px 5px; border-radius: 4px; color: #bac2de; }
</style>
