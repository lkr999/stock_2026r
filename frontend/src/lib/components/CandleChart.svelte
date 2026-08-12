<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { createChart, ColorType, CrosshairMode } from 'lightweight-charts';
  import type { Candle, PatternResult } from '$lib/api';

  export let candles: Candle[] = [];
  export let patterns: PatternResult[] = [];
  export let height = 480;
  export let dataSource: 'live' | '' = '';

  // Korean market convention: 상승=빨강, 하락=파랑
  const UP = '#ef4444';
  const DOWN = '#3b82f6';

  let container: HTMLDivElement;
  let wrapEl: HTMLDivElement;
  let nowLineEl: HTMLDivElement;
  let chart: any;
  let candleSeries: any;
  let volumeSeries: any;
  let initialized = false;
  let lastTimes: number[] = [];
  let nowLabel = '';
  let nowTimer: ReturnType<typeof setInterval>;

  // 캔들 시간축은 'KST 벽시계 → UTC 인코딩' 좌표계 → 현재시각도 동일 인코딩
  function currentKstUnix(): number {
    return Math.floor((Date.now() + 9 * 3600 * 1000) / 1000);
  }

  function updateNowLine() {
    if (!nowLineEl || !chart || lastTimes.length === 0) {
      if (nowLineEl) nowLineEl.style.display = 'none';
      return;
    }
    const nowTs = currentKstUnix();
    const scale = chart.timeScale();
    let x = scale.timeToCoordinate(nowTs as any);
    if (x == null) x = scale.timeToCoordinate(lastTimes[lastTimes.length - 1] as any);
    if (x == null) { nowLineEl.style.display = 'none'; return; }
    nowLineEl.style.display = 'block';
    nowLineEl.style.left = `${x}px`;
    const d = new Date(nowTs * 1000);
    nowLabel = `${String(d.getUTCHours()).padStart(2, '0')}:${String(d.getUTCMinutes()).padStart(2, '0')}`;
  }

  // hover 툴팁
  let tip = { show: false, x: 0, y: 0 };
  let tipRow: { time: string; o: number; h: number; l: number; c: number; v: number; up: boolean } | null = null;
  let timeIndex = new Map<number, number>();

  function toTime(c: Candle, i: number): number {
    const d = (c.date || '').replace(/\D/g, '');
    const t = (c.time || '').replace(/\D/g, '');
    if (d.length === 8) {
      const y = +d.slice(0, 4), mo = +d.slice(4, 6), da = +d.slice(6, 8);
      const hh = t.length >= 4 ? +t.slice(0, 2) : 9;
      const mm = t.length >= 4 ? +t.slice(2, 4) : 0;
      const ms = Date.UTC(y, mo - 1, da, hh, mm) / 1000;
      if (!Number.isNaN(ms)) return ms;
    }
    return 1_700_000_000 + i * 60;
  }

  function fmtTime(c: Candle): string {
    const d = (c.date || '').replace(/\D/g, '');
    const t = (c.time || '').replace(/\D/g, '');
    if (d.length === 8) {
      const base = `${d.slice(0, 4)}-${d.slice(4, 6)}-${d.slice(6, 8)}`;
      return t.length >= 4 ? `${base} ${t.slice(0, 2)}:${t.slice(2, 4)}` : base;
    }
    return c.ts || '';
  }

  function render() {
    if (!candleSeries || candles.length === 0) return;

    const times: number[] = [];
    let prev = 0;
    candles.forEach((c, i) => {
      let t = toTime(c, i);
      if (t <= prev) t = prev + 60;
      times.push(t);
      prev = t;
    });
    lastTimes = times;
    timeIndex = new Map(times.map((t, i) => [t, i]));

    const tscale = chart.timeScale();
    const keepRange = initialized ? tscale.getVisibleLogicalRange() : null;

    candleSeries.setData(
      candles.map((c, i) => ({ time: times[i], open: c.open, high: c.high, low: c.low, close: c.close }))
    );
    volumeSeries.setData(
      candles.map((c, i) => ({
        time: times[i],
        value: c.volume ?? 0,
        color: (c.close >= c.open ? UP : DOWN) + '55'
      }))
    );

    const tsIndex = new Map<string, number>();
    candles.forEach((c, i) => tsIndex.set(c.ts, times[i]));
    const markers = patterns
      .map((p) => {
        const time = tsIndex.get(p.detected_at);
        if (time == null) return null;
        return {
          time,
          position: 'belowBar',
          color: UP,
          shape: 'arrowUp',
          text: `${p.pattern_name} ${Math.round(p.composite_score * 100)}%`
        };
      })
      .filter(Boolean);
    candleSeries.setMarkers(markers);

    // 최초만 전체 맞춤, 이후 갱신은 사용자 확대/축소 유지
    if (!initialized) {
      tscale.fitContent();
      initialized = true;
    } else if (keepRange) {
      tscale.setVisibleLogicalRange(keepRange);
    }
    updateNowLine();
  }

  function onCrosshair(param: any) {
    if (!param.point || !param.time || param.point.x < 0) { tip.show = false; return; }
    const idx = timeIndex.get(param.time as number);
    if (idx == null || idx >= candles.length) { tip.show = false; return; }
    const c = candles[idx];
    tipRow = { time: fmtTime(c), o: c.open, h: c.high, l: c.low, c: c.close, v: c.volume ?? 0, up: c.close >= c.open };
    const rect = wrapEl.getBoundingClientRect();
    let x = param.point.x + 16;
    if (x > rect.width - 170) x = param.point.x - 170;
    tip = { show: true, x, y: 36 };
  }

  onMount(() => {
    chart = createChart(container, {
      width: container.clientWidth,
      height,
      layout: {
        background: { type: ColorType.Solid, color: '#181825' },
        textColor: '#bac2de',
        fontSize: 11,
        fontFamily: '-apple-system, "Segoe UI", sans-serif'
      },
      grid: {
        vertLines: { color: 'rgba(49,50,68,0.4)' },
        horzLines: { color: 'rgba(49,50,68,0.4)' }
      },
      crosshair: {
        mode: CrosshairMode.Normal,
        vertLine: { color: '#a6adc8', width: 1, style: 2, labelBackgroundColor: '#45475a' },
        horzLine: { color: '#a6adc8', width: 1, style: 2, labelBackgroundColor: '#45475a' }
      },
      rightPriceScale: { borderColor: '#313244', scaleMargins: { top: 0.08, bottom: 0.28 } },
      timeScale: { borderColor: '#313244', timeVisible: true, secondsVisible: false, rightOffset: 6 }
    });

    candleSeries = chart.addCandlestickSeries({
      upColor: UP, downColor: DOWN,
      borderUpColor: UP, borderDownColor: DOWN,
      wickUpColor: UP, wickDownColor: DOWN,
      priceLineVisible: false
    });

    volumeSeries = chart.addHistogramSeries({
      priceFormat: { type: 'volume' },
      priceScaleId: 'vol'
    });
    chart.priceScale('vol').applyOptions({ scaleMargins: { top: 0.82, bottom: 0 } });

    chart.subscribeCrosshairMove(onCrosshair);
    chart.timeScale().subscribeVisibleLogicalRangeChange(updateNowLine);
    render();
    const ro = new ResizeObserver(() => { chart?.applyOptions({ width: container.clientWidth }); updateNowLine(); });
    ro.observe(container);
    nowTimer = setInterval(updateNowLine, 1000);
  });

  $: if (candleSeries) {
    candles;
    patterns;
    render();
  }

  onDestroy(() => { clearInterval(nowTimer); chart?.remove(); });
</script>

<div class="chart-wrap" bind:this={wrapEl}>
  <div bind:this={container} class="chart" />
  <div bind:this={nowLineEl} class="nowline" style="display:none; height:{height}px">
    <span class="nowlabel">현재 {nowLabel}</span>
  </div>
  <div class="legend">
    <span class="up">■ 상승</span><span class="down">■ 하락</span>
    <span class="vol">▮ 거래량</span>
    {#if dataSource === 'live'}<span class="live">eBest 실시간</span>{/if}
  </div>

  {#if tip.show && tipRow}
    <div class="tip" style="left:{tip.x}px; top:{tip.y}px">
      <div class="tt-time">{tipRow.time}</div>
      <div class="tt-grid">
        <span>시</span><b>{Math.round(tipRow.o).toLocaleString()}</b>
        <span>고</span><b class="up">{Math.round(tipRow.h).toLocaleString()}</b>
        <span>저</span><b class="down">{Math.round(tipRow.l).toLocaleString()}</b>
        <span>종</span><b class={tipRow.up ? 'up' : 'down'}>{Math.round(tipRow.c).toLocaleString()}</b>
        <span>량</span><b>{Math.round(tipRow.v).toLocaleString()}</b>
      </div>
    </div>
  {/if}
</div>

<style>
  .chart-wrap { position: relative; border-radius: 8px; overflow: hidden; }
  .chart { width: 100%; }
  .nowline {
    position: absolute; top: 0; width: 2.5px;
    background: #f9e2af; z-index: 2; pointer-events: none;
    box-shadow: 0 0 6px rgba(249,226,175,0.7);
  }
  .nowlabel {
    position: absolute; top: 2px; left: 50%; transform: translateX(-50%);
    background: #f9e2af; color: #1e1e2e; font-size: 9px; font-weight: 700;
    padding: 1px 5px; border-radius: 3px; white-space: nowrap;
  }
  .legend {
    position: absolute; top: 8px; left: 12px; z-index: 3;
    display: flex; gap: 12px; font-size: 11px; color: #bac2de;
    pointer-events: none;
  }
  .legend .up { color: #ef4444; }
  .legend .down { color: #3b82f6; }
  .legend .vol { color: #a6adc8; }
  .legend .live { color: #a6e3a1; }
  .tip {
    position: absolute; z-index: 10; pointer-events: none;
    background: rgba(17,17,27,0.95); border: 1px solid #45475a; border-radius: 6px;
    padding: 7px 9px; font-size: 11px; color: #cdd6f4; min-width: 150px;
    box-shadow: 0 4px 14px rgba(0,0,0,0.5);
  }
  .tt-time { color: #bac2de; font-size: 10px; margin-bottom: 4px; }
  .tt-grid { display: grid; grid-template-columns: auto 1fr; gap: 1px 8px; font-variant-numeric: tabular-nums; }
  .tt-grid span { color: #a6adc8; }
  .tt-grid b { text-align: right; }
  .up { color: #ef4444; }
  .down { color: #3b82f6; }
</style>
