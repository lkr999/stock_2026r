<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { api } from '$lib/api';

  let ebestConfigured = true;
  onMount(async () => {
    try { ebestConfigured = (await api.health()).ebest_configured; } catch {}
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
</nav>

<main><slot /></main>

<style>
  :global(body) { margin: 0; background: #11111b; color: #cdd6f4; font-family: -apple-system, 'Segoe UI', sans-serif; }
  nav { display: flex; align-items: center; justify-content: space-between; padding: 12px 24px; background: #181825; border-bottom: 1px solid #313244; }
  .brand { font-weight: 700; font-size: 16px; }
  .nokey { background: #f38ba8; color: #1e1e2e; font-size: 10px; padding: 2px 6px; border-radius: 6px; margin-left: 8px; }
  .links { display: flex; gap: 8px; }
  .links a { color: #bac2de; text-decoration: none; padding: 6px 14px; border-radius: 6px; font-size: 14px; }
  .links a.active { background: #313244; color: #89b4fa; font-weight: 600; }
  main { max-width: 1200px; margin: 0 auto; padding: 24px; }
</style>
