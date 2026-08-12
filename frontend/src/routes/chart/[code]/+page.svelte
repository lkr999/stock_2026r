<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { api, type Candle, type PatternResult } from '$lib/api';
  import { selectedTimeframe, selectedStrategy } from '$lib/stores/timeframe';
  import TimeframeSelector from '$lib/components/TimeframeSelector.svelte';
  import CandleChart from '$lib/components/CandleChart.svelte';
  import CompositeScoreBar from '$lib/components/CompositeScoreBar.svelte';

  $: code = $page.params.code ?? '';
  let candles: Candle[] = [];
  let patterns: PatternResult[] = [];
  let name = '';
  let loading = false;
  let error = '';
  let dataSource: 'live' | '' = '';

  async function load() {
    if (!code) return;
    loading = true; error = '';
    try {
      const [c, p] = await Promise.all([
        api.candles(code, $selectedTimeframe),
        api.patterns(code, $selectedTimeframe, $selectedStrategy, true)
      ]);
      candles = c.candles;
      patterns = p.patterns;
      name = p.name || '';
      dataSource = c.data_source ?? '';
    } catch (e) { error = String(e); }
    loading = false;
  }

  $: code, $selectedTimeframe, $selectedStrategy, load();
  onMount(load);
</script>

<header>
  <div>
    <a href="/" class="back">← 대시보드</a>
    <h1>{name} <span class="code">{code}</span></h1>
  </div>
  <TimeframeSelector />
</header>

{#if error}<div class="error">{error}</div>{/if}

<div class="grid">
  <div class="chart-panel">
    {#if loading}<div class="loading">로딩중…</div>{/if}
    <CandleChart {candles} {patterns} {dataSource} />
  </div>
  <aside>
    <h3>감지된 패턴 ({patterns.length})</h3>
    {#each patterns as p}
      <div class="pcard">
        <div class="phead">
          <span class="bullish">▲ {p.pattern_name}</span>
        </div>
        <CompositeScoreBar
          confidence={p.confidence} mtfScore={p.mtf_score} mlScore={p.ml_score}
          composite={p.composite_score} volConfirmed={p.volume_confirmed} atrNorm={p.atr_normalized} />
        <div class="exp" title="Caginalp-Laurent(1998) 미국 일봉 논문 참고값입니다. 이 종목/타임프레임의 실측 기대값이 아니니, 백테스트(전략)로 확인하세요.">
          참고기대 5/10/25봉(논문*): {p.expected_5}% / {p.expected_10}% / {p.expected_25}%</div>
      </div>
    {/each}
    {#if patterns.length === 0 && !loading}<p class="empty">감지된 패턴 없음</p>{/if}
  </aside>
</div>

<style>
  header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 16px; }
  .back { color: #a6adc8; text-decoration: none; font-size: 13px; }
  h1 { margin: 6px 0 0; font-size: 22px; }
  .code { color: #a6adc8; font-size: 15px; }
  .grid { display: grid; grid-template-columns: 1fr 300px; gap: 16px; }
  .chart-panel { background: #1e1e2e; border-radius: 10px; padding: 12px; position: relative; }
  .loading { position: absolute; inset: 0; display: grid; place-items: center; color: #bac2de; z-index: 2; }
  aside { background: #181825; border-radius: 10px; padding: 16px; }
  aside h3 { margin: 0 0 12px; font-size: 15px; }
  .pcard { margin-bottom: 14px; }
  .phead { margin-bottom: 6px; font-weight: 600; font-size: 14px; }
  .bullish { color: #a6e3a1; }
  .exp { font-size: 11px; color: #a6adc8; margin-top: 6px; }
  .empty { color: #a6adc8; font-size: 13px; }
  .error { background: #f38ba8; color: #1e1e2e; padding: 10px; border-radius: 6px; margin-bottom: 12px; }
  @media (max-width: 800px) { .grid { grid-template-columns: 1fr; } }
</style>
