<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { tradingStatus } from '$lib/stores/tradingStatus';

  let ebestConfigured = true;
  onMount(async () => {
    try { ebestConfigured = (await api.health()).ebest_configured; } catch {}
    tradingStatus.start();
  });

  const links = [
    { href: '/', label: '대시보드' },
    { href: '/trading', label: '자동매매' },
    { href: '/backtest', label: '백테스트' }
  ];
</script>

<nav>
  <div class="brand">📈 Caginalp Trader {#if !ebestConfigured}<span class="nokey" title="eBest 키 미설정 — 데이터 조회 불가">eBest 키 미설정</span>{/if}</div>
  <div class="links">
    {#each links as l}
      <a href={l.href} class:active={$page.url.pathname === l.href}>{l.label}</a>
    {/each}
  </div>
  <a class="run-badge {$tradingStatus.running ? ($tradingStatus.mode === 'live' ? 'live' : 'paper') : 'off'}" href="/trading"
    title="자동매매 실행 상태 (클릭 시 자동매매 페이지로 이동)">
    <span class="dot"></span>
    {#if $tradingStatus.running}
      자동매매 {$tradingStatus.mode === 'live' ? '실전' : '모의'} 실행중 · {($tradingStatus.watchlist ?? []).length}종목
    {:else}
      자동매매 정지
    {/if}
  </a>
</nav>

<main><slot /></main>

<style>
  :global(body) { margin: 0; background: #11111b; color: #cdd6f4; font-family: -apple-system, 'Segoe UI', sans-serif; }
  nav { display: flex; align-items: center; justify-content: space-between; padding: 12px 24px; background: #181825; border-bottom: 1px solid #313244; }
  .brand { font-weight: 700; font-size: 16px; }
  .nokey { background: #f38ba8; color: #1e1e2e; font-size: 10px; padding: 2px 6px; border-radius: 6px; margin-left: 8px; }
  .links { display: flex; gap: 8px; flex: 1; }
  .links a { color: #bac2de; text-decoration: none; padding: 6px 14px; border-radius: 6px; font-size: 14px; }
  .links a.active { background: #313244; color: #89b4fa; font-weight: 600; }
  main { max-width: none; margin: 0; padding: 24px; }
  .run-badge {
    display: flex; align-items: center; gap: 7px; font-size: 12px; font-weight: 600;
    padding: 5px 12px; border-radius: 12px; text-decoration: none; white-space: nowrap;
    background: #313244; color: #bac2de; border: 1px solid #45475a;
  }
  .run-badge .dot { width: 8px; height: 8px; border-radius: 50%; background: #a6adc8; }
  .run-badge.paper { color: #a6e3a1; border-color: #3a4a3a; }
  .run-badge.paper .dot { background: #a6e3a1; box-shadow: 0 0 6px #a6e3a1; }
  .run-badge.live { color: #f38ba8; border-color: #4a2f36; }
  .run-badge.live .dot { background: #f38ba8; box-shadow: 0 0 6px #f38ba8; }
</style>
