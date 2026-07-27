<script lang="ts">
  import { api, type UniverseItem } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';

  export let market = 'ALL';
  export let minPrice: number | undefined = undefined;
  export let maxPrice: number | undefined = undefined;

  let q = '';
  let results: UniverseItem[] = [];
  let total = 0;
  let open = false;
  let timer: ReturnType<typeof setTimeout>;
  let names: Record<string, string> = {};

  // 칩에 종목명 병기를 위해 워치리스트 코드의 이름을 조회
  async function resolveNames(codes: string[]) {
    const missing = codes.filter((c) => !(c in names));
    if (missing.length === 0) return;
    try {
      const r = await api.lookup(missing);
      for (const x of r) names[x.code] = x.name;
      names = names;
    } catch {}
  }
  $: resolveNames($watchlist);

  async function search() {
    const r = await api.universe({ market, minPrice, maxPrice, q, limit: 30 });
    results = r.items;
    total = r.total;
  }
  function onInput() {
    clearTimeout(timer);
    timer = setTimeout(search, 250);
  }
  function focusOpen() {
    open = true;
    if (results.length === 0) search();
  }
</script>

<div class="picker">
  <div class="bar">
    <input
      placeholder="관심종목 검색 (종목명/코드)"
      bind:value={q}
      on:input={onInput}
      on:focus={focusOpen}
    />
    <span class="count">선택 {$watchlist.length}종목</span>
    {#if $watchlist.length > 0}
      <button class="clear" on:click={() => watchlist.clear()}>비우기</button>
    {/if}
  </div>

  {#if $watchlist.length > 0}
    <div class="chips">
      {#each $watchlist as code}
        <span class="chip"><b>{names[code] || code}</b><span class="ccd">{code}</span><button on:click={() => watchlist.remove(code)}>×</button></span>
      {/each}
    </div>
  {/if}

  {#if open}
    <div class="results">
      <div class="rhead">검색 결과 {results.length} / 필터 일치 {total}종목</div>
      {#each results as r}
        <button class="row" class:sel={$watchlist.includes(r.code)} on:click={() => watchlist.toggle(r.code)}>
          <span class="star">{$watchlist.includes(r.code) ? '★' : '☆'}</span>
          <span class="nm">{r.name}</span>
          <span class="cd">{r.code}</span>
          <span class="mk">{r.market}</span>
          <span class="pr">{r.price.toLocaleString()}원</span>
          <span class={r.change_pct >= 0 ? 'up' : 'down'}>{r.change_pct >= 0 ? '+' : ''}{r.change_pct}%</span>
        </button>
      {/each}
      {#if results.length === 0}<div class="empty">결과 없음</div>{/if}
      <button class="close" on:click={() => (open = false)}>닫기</button>
    </div>
  {/if}
</div>

<style>
  .picker { position: relative; }
  .bar { display: flex; align-items: center; gap: 10px; }
  .bar input { flex: 1; background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 6px; padding: 7px 10px; }
  .count { font-size: 12px; color: #bac2de; white-space: nowrap; }
  .clear { background: #313244; color: #cdd6f4; border: none; border-radius: 4px; padding: 5px 10px; cursor: pointer; font-size: 12px; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
  .chip { background: #313244; color: #cdd6f4; border-radius: 12px; padding: 2px 6px 2px 10px; font-size: 12px; display: inline-flex; align-items: center; gap: 5px; }
  .chip .ccd { color: #a6adc8; font-size: 10px; }
  .chip button { background: none; border: none; color: #f38ba8; cursor: pointer; font-size: 14px; line-height: 1; }
  .results { position: absolute; z-index: 20; top: 44px; left: 0; right: 0; background: #11111b; border: 1px solid #45475a; border-radius: 8px; padding: 8px; max-height: 360px; overflow-y: auto; box-shadow: 0 8px 24px rgba(0,0,0,0.5); }
  .rhead { font-size: 11px; color: #a6adc8; padding: 4px 8px; }
  .row { display: grid; grid-template-columns: 24px 1fr 70px 60px 90px 60px; align-items: center; gap: 8px; width: 100%; background: none; border: none; color: #cdd6f4; padding: 7px 8px; cursor: pointer; text-align: left; border-radius: 6px; font-size: 13px; }
  .row:hover { background: #1e1e2e; }
  .row.sel { background: #1e2030; }
  .star { color: #f9e2af; }
  .nm { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cd, .mk { color: #a6adc8; font-size: 11px; }
  .pr { text-align: right; font-variant-numeric: tabular-nums; }
  .up { color: #ef4444; text-align: right; }
  .down { color: #3b82f6; text-align: right; }
  .empty { color: #a6adc8; text-align: center; padding: 16px; font-size: 13px; }
  .close { width: 100%; margin-top: 6px; background: #313244; color: #cdd6f4; border: none; border-radius: 6px; padding: 6px; cursor: pointer; }
</style>
