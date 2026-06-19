<script lang="ts">
  export let confidence = 0;
  export let mtfScore = 0;
  export let mlScore = 0;
  export let composite = 0;
  export let volConfirmed = false;
  export let atrNorm = false;

  $: color = composite >= 0.7 ? '#a6e3a1' : composite >= 0.5 ? '#f9e2af' : '#f38ba8';
  const pct = (v: number) => `${Math.round(v * 100)}%`;
</script>

<div class="score-card">
  <div class="bar-row"><span>규칙</span><div class="bar"><div style="width:{pct(confidence)}; background:#89b4fa" /></div><span>{pct(confidence)}</span></div>
  <div class="bar-row"><span>MTF</span><div class="bar"><div style="width:{pct(mtfScore)}; background:#cba6f7" /></div><span>{pct(mtfScore)}</span></div>
  <div class="bar-row"><span>ML</span><div class="bar"><div style="width:{pct(mlScore)}; background:#fab387" /></div><span>{pct(mlScore)}</span></div>
  <div class="composite" style="color:{color}">
    종합 {pct(composite)}
    {#if volConfirmed}<span class="badge">거래량✓</span>{/if}
    {#if atrNorm}<span class="badge">ATR✓</span>{/if}
  </div>
</div>

<style>
  .score-card { padding: 10px; background: #181825; border-radius: 8px; }
  .bar-row { display: flex; align-items: center; gap: 8px; margin: 3px 0; font-size: 12px; color: #bac2de; }
  .bar-row > span:first-child { width: 36px; }
  .bar-row > span:last-child { width: 36px; text-align: right; }
  .bar { flex: 1; height: 8px; background: #313244; border-radius: 4px; overflow: hidden; }
  .bar div { height: 100%; border-radius: 4px; transition: width 0.3s; }
  .composite { font-weight: 700; margin-top: 8px; font-size: 14px; }
  .badge { background: #313244; padding: 1px 6px; border-radius: 10px; font-size: 10px; margin-left: 4px; color: #cdd6f4; }
</style>
