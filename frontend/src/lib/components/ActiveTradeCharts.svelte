<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, type Candle, type Indicators, type Quote, type Timeframe, type TradeEvent } from '$lib/api';
  import TradingChart from './TradingChart.svelte';

  export let codes: string[] = [];
  export let tf: Timeframe = '5m';
  export let refreshSec = 10;
  export let quoteSec = 5; // 현재가 실시간 폴링 주기(초)
  export let tradeEvents: TradeEvent[] = [];

  let names: Record<string, string> = {};
  let prices: Record<string, number> = {};
  let selected = '';
  let candles: Candle[] = [];
  let indicators: Indicators | null = null;
  let loading = false;
  let fetchError = '';
  let updatedAt = '';
  let timer: ReturnType<typeof setInterval>;

  // 실시간 현재가
  let quote: Quote | null = null;
  let quoteAt = '';
  let quoteTimer: ReturnType<typeof setInterval>;
  let flash = false;

  $: if (codes.length && (!selected || !codes.includes(selected))) selected = codes[0];

  async function loadNames() {
    if (!codes.length) return;
    try {
      const r = await api.lookup(codes);
      for (const x of r) {
        names[x.code] = x.name;
        prices[x.code] = x.price;
      }
      names = names;
    } catch {}
  }

  async function loadChart() {
    if (!selected) {
      candles = [];
      indicators = null;
      fetchError = '';
      return;
    }
    loading = true;
    fetchError = '';
    try {
      const r = await api.indicators(selected, tf);
      candles = r.candles;
      indicators = r.indicators;
      if (r.name) names[selected] = r.name;
      updatedAt = new Date().toLocaleTimeString('ko-KR');
    } catch (e: any) {
      fetchError = String(e?.message ?? e);
      candles = [];
      indicators = null;
    }
    loading = false;
  }

  async function loadQuote() {
    if (!selected) { quote = null; return; }
    try {
      // 차트와 동일한 타임프레임으로 조회해야 헤더 현재가가 차트 마지막 봉과 일치
      const q = await api.quote(selected, tf);
      if (quote && q.price !== quote.price) {
        flash = true;
        setTimeout(() => (flash = false), 400);
      }
      quote = q;
      prices[selected] = q.price;
      quoteAt = new Date().toLocaleTimeString('ko-KR');
    } catch {
      /* 현재가 폴링 실패는 차트 갱신과 별개로 조용히 무시 */
    }
  }

  $: selected, tf, loadChart();
  $: selected, tf, loadQuote();
  $: codes, loadNames();

  // 현재 선택 종목의 거래 이벤트만 필터링
  $: currentTrades = tradeEvents.filter(e => e.code === selected);

  onMount(() => {
    loadNames();
    loadChart();
    loadQuote();
    timer = setInterval(loadChart, refreshSec * 1000);
    quoteTimer = setInterval(loadQuote, quoteSec * 1000);
  });
  onDestroy(() => { clearInterval(timer); clearInterval(quoteTimer); });
</script>

<div class="atc">
  {#if codes.length === 0}
    <p class="empty">자동매매 대상 종목이 없습니다. 관심종목을 추가하고 매매를 시작하세요.</p>
  {:else}
    <div class="tabs">
      {#each codes as c}
        <button class:active={selected === c} on:click={() => (selected = c)}>
          <strong>{names[c] || c}</strong><span class="cd">{c}</span>
        </button>
      {/each}
      <span class="meta">{loading ? '갱신중…' : updatedAt ? `${updatedAt} 갱신` : ''} · {refreshSec}s 주기</span>
    </div>

    {#if selected}
      <div class="head">
        <span class="title">
          {names[selected] || selected} <span class="cd">{selected}</span>
        </span>
        {#if quote}
          <span class="quote">
            <span class="price" class:flash class:up={quote.change >= 0} class:down={quote.change < 0}>
              {Math.round(quote.price).toLocaleString()}원
            </span>
            <span class="chg" class:up={quote.change >= 0} class:down={quote.change < 0}>
              {quote.change >= 0 ? '▲' : '▼'} {Math.abs(Math.round(quote.change)).toLocaleString()}
              ({quote.change >= 0 ? '+' : ''}{quote.change_pct}%)
            </span>
            <span class="qtime">⟳ {quoteAt} · {quoteSec}s</span>
          </span>
        {:else if prices[selected]}
          <span class="price">{prices[selected].toLocaleString()}원</span>
        {/if}
      </div>
    {/if}

    {#if fetchError}
      <div class="fetch-error">
        <strong>데이터 조회 오류</strong> — eBest API 연결을 확인하세요.<br>
        <span class="err-detail">{fetchError}</span>
      </div>
    {:else}
      <TradingChart {candles} {indicators} trades={currentTrades} resetKey={selected} />
    {/if}
  {/if}
</div>

<style>
  .atc { }
  .tabs { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; margin-bottom: 8px; }
  .tabs button { display: flex; flex-direction: column; align-items: flex-start; background: #1e1e2e; border: 1px solid #45475a; color: #cdd6f4; border-radius: 6px; padding: 4px 10px; cursor: pointer; line-height: 1.2; }
  .tabs button.active { background: #313244; border-color: #89b4fa; }
  .tabs button strong { font-size: 13px; }
  .cd { color: #a6adc8; font-size: 10px; }
  .meta { margin-left: auto; font-size: 11px; color: #a6adc8; }
  .head { display: flex; justify-content: space-between; align-items: baseline; padding: 4px 4px 8px; }
  .title { font-weight: 700; font-size: 15px; display: flex; align-items: center; gap: 8px; }
  .quote { display: inline-flex; align-items: baseline; gap: 10px; }
  .price { font-variant-numeric: tabular-nums; color: #bac2de; font-weight: 700; font-size: 16px; transition: background 0.2s; border-radius: 4px; padding: 0 4px; }
  .price.flash { background: rgba(249,226,175,0.35); }
  .chg { font-size: 12px; font-variant-numeric: tabular-nums; }
  .qtime { font-size: 10px; color: #a6adc8; }
  .up { color: #ef4444; }
  .down { color: #3b82f6; }
  .empty { color: #a6adc8; font-size: 13px; text-align: center; padding: 24px; }
  .fetch-error {
    background: #f38ba820; border: 1px solid #f38ba8; border-radius: 8px;
    padding: 16px; color: #f38ba8; font-size: 13px; margin: 8px 0;
  }
  .err-detail { font-size: 11px; color: #bac2de; word-break: break-all; }
</style>
