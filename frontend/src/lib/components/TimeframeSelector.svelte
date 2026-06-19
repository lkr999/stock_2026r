<script lang="ts">
  import { selectedTimeframe, TIMEFRAME_LABELS } from '$lib/stores/timeframe';
  import type { Timeframe } from '$lib/api';
  const timeframes = Object.keys(TIMEFRAME_LABELS) as Timeframe[];
</script>

<div class="tf-selector">
  {#each timeframes as tf}
    <button class:active={$selectedTimeframe === tf} on:click={() => selectedTimeframe.set(tf)}>
      {TIMEFRAME_LABELS[tf]}
    </button>
  {/each}
</div>

<style>
  .tf-selector { display: flex; gap: 4px; flex-wrap: wrap; }
  button {
    padding: 4px 10px; border: 1px solid #45475a; background: #1e1e2e;
    color: #cdd6f4; border-radius: 4px; cursor: pointer; font-size: 13px;
  }
  button.active { background: #89b4fa; color: #1e1e2e; border-color: #89b4fa; font-weight: 600; }
</style>
