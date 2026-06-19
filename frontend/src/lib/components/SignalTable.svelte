<script lang="ts">
  import type { SignalItem } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';
  export let signals: SignalItem[] = [];
  export let loading = false;
</script>

<table>
  <thead>
    <tr>
      <th></th><th>종목</th><th>현재가</th><th>등락</th><th>패턴</th><th>방향</th>
      <th>신뢰도</th><th>종합</th><th>확인</th><th>기대(5봉)</th><th></th>
    </tr>
  </thead>
  <tbody>
    {#each signals as s}
      <tr class:watched={$watchlist.includes(s.shcode)}>
        <td>
          <button class="star" class:on={$watchlist.includes(s.shcode)} on:click={() => watchlist.toggle(s.shcode)}
            title="관심종목 추가/제거">{$watchlist.includes(s.shcode) ? '★' : '☆'}</button>
        </td>
        <td><strong>{s.name}</strong><span class="code">{s.shcode}</span></td>
        <td class="num">{(s.price ?? 0).toLocaleString()}</td>
        <td class="num {(s.change_pct ?? 0) >= 0 ? 'up' : 'down'}">{(s.change_pct ?? 0) >= 0 ? '+' : ''}{s.change_pct ?? 0}%</td>
        <td>{s.pattern_name}</td>
        <td class={s.pattern_type}>{s.pattern_type === 'bullish' ? '▲ 상승' : '▼ 하락'}</td>
        <td class="num">{Math.round(s.confidence * 100)}%</td>
        <td><span class="score" class:hi={s.composite_score >= 0.7}>{Math.round(s.composite_score * 100)}%</span></td>
        <td class="conf">
          {#if s.atr_normalized}<span class="b" title="몸통 ≥ ATR×0.3">ATR</span>{/if}
          {#if s.volume_confirmed}<span class="b" title="거래량 ≥ 평균×1.5">VOL</span>{/if}
        </td>
        <td class="num {(s.expected_5 ?? 0) >= 0 ? 'up' : 'down'}">{(s.expected_5 ?? 0).toFixed(2)}%</td>
        <td><a href={`/chart/${s.shcode}`}>차트</a></td>
      </tr>
    {/each}
    {#if signals.length === 0}
      <tr><td colspan="11" class="empty">
        {#if loading}종목 스캔 중… (종목당 약 1초 · eBest 호출 한도)
        {:else}신호 없음 — 타임프레임/전략/가격범위를 바꿔보세요{/if}
      </td></tr>
    {/if}
  </tbody>
</table>

<style>
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th { text-align: left; padding: 8px; color: #9399b2; border-bottom: 1px solid #313244; font-weight: 500; }
  td { padding: 8px; border-bottom: 1px solid #232334; color: #cdd6f4; }
  tr.watched { background: #1a1a2a; }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  .code { color: #6c7086; margin-left: 6px; font-size: 11px; }
  .up { color: #ef4444; }
  .down { color: #3b82f6; }
  .bullish { color: #ef4444; }
  .bearish { color: #3b82f6; }
  .score { padding: 2px 6px; border-radius: 4px; background: #313244; }
  .score.hi { background: #a6e3a1; color: #1e1e2e; font-weight: 700; }
  .conf .b { font-size: 10px; background: #313244; padding: 1px 5px; border-radius: 8px; margin-right: 3px; }
  .empty { text-align: center; color: #6c7086; padding: 24px; }
  .star { background: none; border: none; cursor: pointer; font-size: 16px; color: #6c7086; padding: 0; }
  .star.on { color: #f9e2af; }
  a { color: #89b4fa; text-decoration: none; }
</style>
