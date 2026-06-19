<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { createChart, ColorType, CrosshairMode, LineStyle } from 'lightweight-charts';
  import type { Candle, Indicators, TradeEvent } from '$lib/api';

  export let candles: Candle[] = [];
  export let indicators: Indicators | null = null;
  export let trades: TradeEvent[] = [];
  export let resetKey = ''; // 값이 바뀌면(종목 전환 등) 확대상태를 초기화하고 전체 맞춤

  const UP = '#ef4444'; // 상승=빨강
  const DOWN = '#3b82f6'; // 하락=파랑

  const MAIN_H = 280;

  let mainEl: HTMLDivElement;
  let rsiEl: HTMLDivElement;
  let macdEl: HTMLDivElement;
  let wrapEl: HTMLDivElement;
  let nowLineEl: HTMLDivElement;
  let mainChart: any, rsiChart: any, macdChart: any;
  let candleS: any, volS: any, ma5S: any, ma20S: any, ma60S: any, bbU: any, bbM: any, bbL: any;
  let rsiS: any, macdS: any, sigS: any, histS: any;
  let ro: ResizeObserver;
  let initialized = false;
  let lastTimes: number[] = [];
  let nowLabel = '';
  let nowTimer: ReturnType<typeof setInterval>;

  // 캔들 시간축은 'KST 벽시계 → UTC 인코딩'(Date.UTC) 좌표계를 쓰므로,
  // 현재 시각도 동일하게 인코딩해야 세로선이 올바른 위치에 온다.
  function currentKstUnix(): number {
    return Math.floor((Date.now() + 9 * 3600 * 1000) / 1000);
  }

  // hover 툴팁 상태
  let tip = { show: false, x: 0, y: 0 };
  let tipRow: { time: string; o: number; h: number; l: number; c: number; v: number; up: boolean;
    ma5?: number | null; ma20?: number | null; ma60?: number | null;
    rsi?: number | null; macd?: number | null; sig?: number | null } | null = null;
  let timeIndex = new Map<number, number>();

  function toTimes(): number[] {
    const times: number[] = [];
    let prev = 0;
    candles.forEach((c, i) => {
      const d = (c.date || '').replace(/\D/g, '');
      const t = (c.time || '').replace(/\D/g, '');
      let ts: number;
      if (d.length === 8) {
        const hh = t.length >= 4 ? +t.slice(0, 2) : 9;
        const mm = t.length >= 4 ? +t.slice(2, 4) : 0;
        ts = Date.UTC(+d.slice(0, 4), +d.slice(4, 6) - 1, +d.slice(6, 8), hh, mm) / 1000;
      } else {
        ts = 1_700_000_000 + i * 60;
      }
      if (ts <= prev) ts = prev + 60;
      times.push(ts);
      prev = ts;
    });
    return times;
  }

  function line(arr: (number | null)[] | undefined, times: number[]) {
    if (!arr) return [];
    const out: { time: number; value: number }[] = [];
    arr.forEach((v, i) => {
      if (v != null && i < times.length) out.push({ time: times[i] as any, value: v });
    });
    return out;
  }

  function buildMarkers(times: number[]): any[] {
    if (!trades.length || !times.length) return [];
    const tMin = times[0];
    const tMax = times[times.length - 1];
    const markers: any[] = [];
    const seen = new Set<number>();

    for (const ev of trades) {
      let best = times[0];
      let bestDiff = Math.abs(times[0] - ev.ts);
      for (const t of times) {
        const diff = Math.abs(t - ev.ts);
        if (diff < bestDiff) { bestDiff = diff; best = t; }
      }
      if (ev.ts < tMin - 86400 || ev.ts > tMax + 86400) continue;

      let markerTs = best;
      while (seen.has(markerTs)) markerTs++;
      seen.add(markerTs);

      const isBuy = ev.type === 'buy';
      const pnlText = ev.pnl !== 0
        ? ` (${ev.pnl >= 0 ? '+' : ''}${Math.round(ev.pnl).toLocaleString()}원 ${ev.pnl_pct >= 0 ? '+' : ''}${ev.pnl_pct.toFixed(1)}%)`
        : '';
      markers.push({
        time: markerTs as any,
        position: isBuy ? 'belowBar' : 'aboveBar',
        color: isBuy ? '#a6e3a1' : '#f38ba8',
        shape: isBuy ? 'arrowUp' : 'arrowDown',
        text: `${isBuy ? '매수' : '매도'} ${ev.qty.toLocaleString()}주@${Math.round(ev.price).toLocaleString()}${pnlText}`,
      });
    }
    markers.sort((a, b) => (a.time as number) - (b.time as number));
    return markers;
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

  function updateNowLine() {
    if (!nowLineEl || !mainChart || lastTimes.length === 0) {
      if (nowLineEl) nowLineEl.style.display = 'none';
      return;
    }
    const nowTs = currentKstUnix();
    const scale = mainChart.timeScale();
    // 현재시각이 데이터 범위 밖(미래)이면 마지막 봉 위치에 표시
    let x = scale.timeToCoordinate(nowTs as any);
    if (x == null) x = scale.timeToCoordinate(lastTimes[lastTimes.length - 1] as any);
    if (x == null) { nowLineEl.style.display = 'none'; return; }
    nowLineEl.style.display = 'block';
    nowLineEl.style.left = `${x}px`;
    const d = new Date(nowTs * 1000); // UTC 구성요소 = KST 벽시계
    nowLabel = `${String(d.getUTCHours()).padStart(2, '0')}:${String(d.getUTCMinutes()).padStart(2, '0')}`;
  }

  function render() {
    if (!candleS || candles.length === 0) return;
    const times = toTimes();
    lastTimes = times;
    timeIndex = new Map(times.map((t, i) => [t, i]));

    const ts = mainChart.timeScale();
    const keepRange = initialized ? ts.getVisibleLogicalRange() : null;

    candleS.setData(
      candles.map((c, i) => ({ time: times[i] as any, open: c.open, high: c.high, low: c.low, close: c.close }))
    );
    volS.setData(
      candles.map((c, i) => ({ time: times[i] as any, value: c.volume ?? 0, color: (c.close >= c.open ? UP : DOWN) + '40' }))
    );

    if (indicators) {
      ma5S.setData(line(indicators.ma5, times));
      ma20S.setData(line(indicators.ma20, times));
      ma60S.setData(line(indicators.ma60, times));
      bbU.setData(line(indicators.bb_upper, times));
      bbM.setData(line(indicators.bb_mid, times));
      bbL.setData(line(indicators.bb_lower, times));
      rsiS.setData(line(indicators.rsi14, times));
      macdS.setData(line(indicators.macd, times));
      sigS.setData(line(indicators.macd_signal, times));
      histS.setData(
        (indicators.macd_hist || [])
          .map((v, i) => (v == null ? null : { time: times[i] as any, value: v, color: v >= 0 ? UP + '99' : DOWN + '99' }))
          .filter(Boolean) as any
      );
    }
    candleS.setMarkers(buildMarkers(times));

    // 최초 1회만 전체 맞춤. 이후 갱신은 사용자가 보던 확대/축소 영역을 유지.
    if (!initialized) {
      ts.fitContent();
      initialized = true;
    } else if (keepRange) {
      ts.setVisibleLogicalRange(keepRange);
    }
    updateNowLine();
  }

  function baseOpts(el: HTMLDivElement, h: number) {
    return {
      width: el.clientWidth,
      height: h,
      layout: { background: { type: ColorType.Solid, color: '#181825' }, textColor: '#9399b2', fontSize: 10 },
      grid: { vertLines: { color: 'rgba(49,50,68,0.3)' }, horzLines: { color: 'rgba(49,50,68,0.3)' } },
      crosshair: { mode: CrosshairMode.Normal },
      rightPriceScale: { borderColor: '#313244' },
      timeScale: { borderColor: '#313244', timeVisible: true, secondsVisible: false }
    };
  }

  function sync(a: any, b: any) {
    a.timeScale().subscribeVisibleLogicalRangeChange((r: any) => r && b.timeScale().setVisibleLogicalRange(r));
  }

  function onCrosshair(param: any) {
    if (!param.point || !param.time || param.point.x < 0) {
      tip.show = false;
      return;
    }
    const idx = timeIndex.get(param.time as number);
    if (idx == null || idx >= candles.length) {
      tip.show = false;
      return;
    }
    const c = candles[idx];
    tipRow = {
      time: fmtTime(c), o: c.open, h: c.high, l: c.low, c: c.close,
      v: c.volume ?? 0, up: c.close >= c.open,
      ma5: indicators?.ma5?.[idx], ma20: indicators?.ma20?.[idx], ma60: indicators?.ma60?.[idx],
      rsi: indicators?.rsi14?.[idx], macd: indicators?.macd?.[idx], sig: indicators?.macd_signal?.[idx],
    };
    const rect = wrapEl.getBoundingClientRect();
    let x = param.point.x + 16;
    if (x > rect.width - 180) x = param.point.x - 180;
    tip = { show: true, x, y: 8 };
  }

  onMount(() => {
    mainChart = createChart(mainEl, baseOpts(mainEl, MAIN_H));
    candleS = mainChart.addCandlestickSeries({
      upColor: UP, downColor: DOWN, borderUpColor: UP, borderDownColor: DOWN,
      wickUpColor: UP, wickDownColor: DOWN, priceLineVisible: false
    });
    bbU = mainChart.addLineSeries({ color: 'rgba(148,163,184,0.5)', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
    bbM = mainChart.addLineSeries({ color: 'rgba(148,163,184,0.35)', lineWidth: 1, lineStyle: LineStyle.Dotted, priceLineVisible: false, lastValueVisible: false });
    bbL = mainChart.addLineSeries({ color: 'rgba(148,163,184,0.5)', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
    ma5S = mainChart.addLineSeries({ color: '#f9e2af', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
    ma20S = mainChart.addLineSeries({ color: '#89b4fa', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
    ma60S = mainChart.addLineSeries({ color: '#cba6f7', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
    volS = mainChart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: 'vol' });
    mainChart.priceScale('vol').applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });

    rsiChart = createChart(rsiEl, baseOpts(rsiEl, 96));
    rsiS = rsiChart.addLineSeries({ color: '#94e2d5', lineWidth: 1, priceLineVisible: false });
    rsiS.createPriceLine({ price: 70, color: '#f38ba8', lineWidth: 1, lineStyle: LineStyle.Dashed });
    rsiS.createPriceLine({ price: 30, color: '#a6e3a1', lineWidth: 1, lineStyle: LineStyle.Dashed });

    macdChart = createChart(macdEl, baseOpts(macdEl, 96));
    histS = macdChart.addHistogramSeries({ priceLineVisible: false });
    macdS = macdChart.addLineSeries({ color: '#89b4fa', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
    sigS = macdChart.addLineSeries({ color: '#fab387', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });

    sync(mainChart, rsiChart); sync(mainChart, macdChart);
    sync(rsiChart, mainChart); sync(macdChart, mainChart);
    mainChart.subscribeCrosshairMove(onCrosshair);
    // 스크롤/줌 시 현재시각 세로선 위치 갱신
    mainChart.timeScale().subscribeVisibleLogicalRangeChange(updateNowLine);

    render();
    ro = new ResizeObserver(() => {
      mainChart.applyOptions({ width: mainEl.clientWidth });
      rsiChart.applyOptions({ width: rsiEl.clientWidth });
      macdChart.applyOptions({ width: macdEl.clientWidth });
      updateNowLine();
    });
    ro.observe(mainEl);
    // 현재시각 진행에 따라 세로선이 흐르도록 1초마다 갱신
    nowTimer = setInterval(updateNowLine, 1000);
  });

  // 종목 전환 시 확대상태 초기화 (render 전에 평가되도록 먼저 선언)
  let prevReset = resetKey;
  $: if (resetKey !== prevReset) { prevReset = resetKey; initialized = false; }

  $: if (candleS) {
    candles; indicators; trades;
    render();
  }

  onDestroy(() => {
    clearInterval(nowTimer);
    ro?.disconnect();
    mainChart?.remove(); rsiChart?.remove(); macdChart?.remove();
  });
</script>

<div class="tc" bind:this={wrapEl}>
  <div class="legend">
    <span style="color:#f9e2af">MA5</span><span style="color:#89b4fa">MA20</span>
    <span style="color:#cba6f7">MA60</span><span style="color:#94a3b8">볼린저</span>
    <span style="color:#a6e3a1">▲ 매수</span><span style="color:#f38ba8">▼ 매도</span>
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
      <div class="tt-ind">
        {#if tipRow.ma5 != null}<span style="color:#f9e2af">MA5 {Math.round(tipRow.ma5).toLocaleString()}</span>{/if}
        {#if tipRow.ma20 != null}<span style="color:#89b4fa">MA20 {Math.round(tipRow.ma20).toLocaleString()}</span>{/if}
        {#if tipRow.ma60 != null}<span style="color:#cba6f7">MA60 {Math.round(tipRow.ma60).toLocaleString()}</span>{/if}
        {#if tipRow.rsi != null}<span style="color:#94e2d5">RSI {tipRow.rsi.toFixed(1)}</span>{/if}
        {#if tipRow.macd != null}<span style="color:#89b4fa">MACD {tipRow.macd.toFixed(1)}</span>{/if}
      </div>
    </div>
  {/if}

  <div class="main-wrap">
    <div bind:this={mainEl} class="pane" />
    <div bind:this={nowLineEl} class="nowline" style="display:none">
      <span class="nowlabel">현재 {nowLabel}</span>
    </div>
  </div>
  <div class="plabel">RSI(14)</div>
  <div bind:this={rsiEl} class="pane" />
  <div class="plabel">MACD(12,26,9)</div>
  <div bind:this={macdEl} class="pane" />
</div>

<style>
  .tc { background: #181825; border-radius: 8px; padding: 6px; position: relative; }
  .pane { width: 100%; }
  .main-wrap { position: relative; }
  .nowline {
    position: absolute; top: 0; height: 280px; width: 2.5px;
    background: #f9e2af; z-index: 4; pointer-events: none;
    box-shadow: 0 0 6px rgba(249,226,175,0.7);
  }
  .nowlabel {
    position: absolute; top: 2px; left: 50%; transform: translateX(-50%);
    background: #f9e2af; color: #1e1e2e; font-size: 9px; font-weight: 700;
    padding: 1px 5px; border-radius: 3px; white-space: nowrap;
  }
  .legend { display: flex; gap: 10px; font-size: 10px; padding: 2px 4px; flex-wrap: wrap; }
  .plabel { font-size: 10px; color: #6c7086; padding: 4px 4px 0; }
  .tip {
    position: absolute; z-index: 10; pointer-events: none;
    background: rgba(17,17,27,0.95); border: 1px solid #45475a; border-radius: 6px;
    padding: 7px 9px; font-size: 11px; color: #cdd6f4; min-width: 150px;
    box-shadow: 0 4px 14px rgba(0,0,0,0.5);
  }
  .tt-time { color: #9399b2; font-size: 10px; margin-bottom: 4px; }
  .tt-grid { display: grid; grid-template-columns: auto 1fr; gap: 1px 8px; font-variant-numeric: tabular-nums; }
  .tt-grid span { color: #6c7086; }
  .tt-grid b { text-align: right; }
  .tt-ind { margin-top: 5px; display: flex; flex-direction: column; gap: 1px; font-size: 10px; font-variant-numeric: tabular-nums; }
  .up { color: #ef4444; }
  .down { color: #3b82f6; }
</style>
