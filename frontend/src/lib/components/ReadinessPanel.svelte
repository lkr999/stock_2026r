<script lang="ts">
  import type { ReadinessReport } from '$lib/api';
  export let report: ReadinessReport | null = null;

  function fmt(key: string, v: number): string {
    if (key === 'win_rate') return `${(v * 100).toFixed(0)}%`;
    if (key === 'pnl') return `${Math.round(v).toLocaleString()}원`;
    return `${v}`;
  }
</script>

<div class="ready-card" class:ok={report?.ready}>
  <div class="head">
    <span class="title">실전 전환 검증 (참고)</span>
    {#if report}
      <span class="badge" class:on={report.ready}>{report.ready ? '✓ 검증 완료' : '검증 미달'}</span>
    {/if}
  </div>
  {#if report}
    <p class="desc">
      {report.ready
        ? '모든 기준을 충족했습니다. 실전투자를 권장할 수 있는 상태입니다.'
        : '아래는 실전투자 권장 기준입니다. 모드는 자유롭게 선택할 수 있으나, 모의투자로 충분히 검증하는 것을 권장합니다.'}
    </p>
    <ul>
      {#each report.criteria as c}
        <li class:pass={c.passed}>
          <span class="ic">{c.passed ? '✓' : '○'}</span>
          <span class="lbl">{c.label}</span>
          <span class="val">{fmt(c.key, c.actual)} <span class="req">/ {fmt(c.key, c.required)}</span></span>
        </li>
      {/each}
    </ul>
    <div class="stats">
      페이퍼 거래 {report.stats.trades}건 · 승률 {(report.stats.win_rate * 100).toFixed(0)}% ·
      PF {report.stats.profit_factor === 999 ? '∞' : report.stats.profit_factor?.toFixed(2)} ·
      누적 {Math.round(report.stats.total_pnl ?? 0).toLocaleString()}원
    </div>
  {:else}
    <p class="desc">불러오는 중…</p>
  {/if}
</div>

<style>
  .ready-card { background: #181825; border: 1px solid #45475a; border-radius: 10px; padding: 16px; }
  .ready-card.ok { border-color: #a6e3a1; }
  .head { display: flex; justify-content: space-between; align-items: center; }
  .title { font-weight: 700; font-size: 15px; }
  .badge { font-size: 12px; padding: 3px 10px; border-radius: 12px; background: #f38ba8; color: #1e1e2e; font-weight: 700; }
  .badge.on { background: #a6e3a1; }
  .desc { font-size: 12px; color: #bac2de; margin: 8px 0 12px; }
  ul { list-style: none; padding: 0; margin: 0; }
  li { display: flex; align-items: center; gap: 8px; padding: 5px 0; font-size: 13px; color: #bac2de; border-bottom: 1px solid #232334; }
  li.pass { color: #cdd6f4; }
  .ic { width: 18px; color: #a6adc8; }
  li.pass .ic { color: #a6e3a1; }
  .lbl { flex: 1; }
  .val { font-variant-numeric: tabular-nums; }
  .req { color: #a6adc8; font-size: 11px; }
  .stats { margin-top: 12px; font-size: 11px; color: #a6adc8; }
</style>
