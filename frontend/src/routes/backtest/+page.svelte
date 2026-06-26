<script lang="ts">
  import { get } from 'svelte/store';
  import { api } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';

  let shcode = '005930';
  let tf = '1d';
  let holdBars = 5;
  let strategy = '';                 // '' = 전체 패턴
  let result: any = null;
  let compare: any = null;
  let batch: any = null;
  let verify: any = null;
  let loading = false;
  let error = '';

  const presets = ['conservative', 'balanced', 'aggressive', 'ml_blended'];

  async function run() {
    loading = true; error = ''; result = null;
    try {
      result = await api.backtest({ shcode, tf, hold_bars: holdBars });
    } catch (e) { error = String(e); }
    loading = false;
  }
  async function runCompare() {
    loading = true; error = ''; compare = null;
    try {
      compare = await api.compareStrategies({ shcode, tf, hold_bars: holdBars });
    } catch (e) { error = String(e); }
    loading = false;
  }
  // 자동매매 관심종목 전체 백테스트
  async function runBatch() {
    const shcodes = get(watchlist);
    if (!shcodes.length) { error = '관심종목이 비어 있습니다. 대시보드에서 자동매매 종목을 먼저 선택하세요.'; return; }
    loading = true; error = ''; batch = null;
    try {
      batch = await api.batchBacktest({
        shcodes, tf, hold_bars: holdBars, strategy: strategy || undefined
      });
    } catch (e) { error = String(e); }
    loading = false;
  }
  // 실매매-동일 조건으로 관심종목 검증 → tradeable 종목만 선별
  async function runVerify() {
    const shcodes = get(watchlist);
    if (!shcodes.length) { error = '관심종목이 비어 있습니다. 대시보드에서 자동매매 종목을 먼저 선택하세요.'; return; }
    loading = true; error = ''; verify = null;
    try {
      verify = await api.strategyBatch({ shcodes, tf, strategy: strategy || 'balanced' });
    } catch (e) { error = String(e); }
    loading = false;
  }
  // 검증 통과(selected) 종목만 관심종목으로 교체
  function applySelected() {
    if (!verify?.selected?.length) return;
    watchlist.clear();
    for (const code of verify.selected) watchlist.add(code);
  }
</script>

<h1>백테스트 (거래비용 차감)</h1>
<p class="sub">진입은 다음 봉 시가, 청산은 N봉 후 종가. 왕복 비용(수수료+세금+슬리피지)을 차감한 순수익입니다.</p>

<div class="controls">
  <div class="group"><label>종목코드</label><input bind:value={shcode} /></div>
  <div class="group"><label>타임프레임</label>
    <select bind:value={tf}>{#each ['5m','15m','30m','60m','1d'] as t}<option>{t}</option>{/each}</select></div>
  <div class="group"><label>보유봉수</label>
    <select bind:value={holdBars}>{#each [5,10,25] as h}<option value={h}>{h}</option>{/each}</select></div>
  <div class="group"><label>전략(일괄용)</label>
    <select bind:value={strategy}>
      <option value="">전체 패턴</option>
      {#each presets as p}<option value={p}>{p}</option>{/each}
    </select></div>
  <button on:click={run} disabled={loading}>실행</button>
  <button class="alt" on:click={runCompare} disabled={loading}>전략 비교</button>
  <button class="batch" on:click={runBatch} disabled={loading} title="대시보드에서 선택한 자동매매 관심종목 전체를 백테스트">
    ★ 관심종목 전체 ({$watchlist.length})
  </button>
  <button class="verify" on:click={runVerify} disabled={loading} title="실매매와 동일한 조건(임계값·비용·ATR청산)으로 OOS 검증해 통과 종목만 선별">
    ✓ 검증 통과 선별 ({$watchlist.length})
  </button>
</div>

{#if error}<div class="error">{error}</div>{/if}
{#if loading}<p>계산중…</p>{/if}

{#if result}
  <div class="panel">
    <h3>요약 — {result.shcode} ({result.timeframe}, {result.hold_bars}봉)</h3>
    <div class="metrics">
      <div><span>총 신호</span><strong>{result.total_signals}</strong></div>
      <div><span>승률</span><strong>{(result.win_rate * 100).toFixed(1)}%</strong></div>
      <div><span>평균순수익</span><strong class={result.avg_return >= 0 ? 'pos' : 'neg'}>{result.avg_return.toFixed(3)}%</strong></div>
      <div><span>왕복비용</span><strong>{result.round_trip_cost_pct.toFixed(3)}%</strong></div>
    </div>
    <table>
      <thead><tr><th>패턴</th><th>신호</th><th>승률</th><th>평균순수익</th><th>PF</th><th>MDD</th></tr></thead>
      <tbody>
        {#each result.by_pattern.filter((b) => b.signals > 0) as b}
          <tr><td>{b.pattern}</td><td>{b.signals}</td><td>{(b.win_rate*100).toFixed(0)}%</td>
            <td class={b.avg_return>=0?'pos':'neg'}>{b.avg_return.toFixed(2)}%</td>
            <td class={b.profit_factor>=1.3?'pos':''}>{b.profit_factor.toFixed(2)}</td>
            <td class="neg">{b.max_drawdown.toFixed(1)}%</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

{#if batch}
  <div class="panel">
    <h3>관심종목 전체 백테스트 — {batch.count}종목 ({batch.timeframe}, {batch.hold_bars}봉{#if batch.strategy}, {batch.strategy}{/if})</h3>
    <div class="metrics">
      <div><span>대상 종목</span><strong>{batch.count}</strong></div>
      <div><span>신호 발생 종목</span><strong>{batch.graded_count}</strong></div>
      <div><span>총 신호</span><strong>{batch.aggregate.total_signals}</strong></div>
      <div><span>가중 승률</span><strong>{(batch.aggregate.win_rate * 100).toFixed(1)}%</strong></div>
      <div><span>가중 평균순수익</span><strong class={batch.aggregate.avg_return >= 0 ? 'pos' : 'neg'}>{batch.aggregate.avg_return.toFixed(3)}%</strong></div>
      <div><span>왕복비용</span><strong>{batch.round_trip_cost_pct.toFixed(3)}%</strong></div>
    </div>
    <table>
      <thead><tr><th>종목</th><th>코드</th><th>신호</th><th>승률</th><th>평균순수익</th><th>최고 패턴</th><th>비고</th></tr></thead>
      <tbody>
        {#each batch.items as it}
          {@const best = (it.by_pattern || []).filter((b) => b.signals > 0).sort((a, b) => b.avg_return - a.avg_return)[0]}
          <tr class:dim={!it.ok || it.total_signals === 0}>
            <td><strong>{it.name || '-'}</strong></td>
            <td class="cd">{it.shcode}</td>
            <td>{it.total_signals}</td>
            <td>{it.total_signals > 0 ? (it.win_rate * 100).toFixed(0) + '%' : '-'}</td>
            <td class={it.avg_return >= 0 ? 'pos' : 'neg'}>{it.total_signals > 0 ? it.avg_return.toFixed(2) + '%' : '-'}</td>
            <td>{best ? `${best.pattern} (${best.avg_return.toFixed(1)}%)` : '-'}</td>
            <td class="note">{it.ok ? (it.total_signals === 0 ? '신호 없음' : '') : it.error}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

{#if verify}
  <div class="panel">
    <h3>실매매-동일 검증 (OOS) — {verify.count}종목 · {verify.strategy} · {verify.timeframe}</h3>
    <p class="sub">진입 임계값·거래량/비용/손익비 게이트·ATR 손절/익절/트레일링을 그대로 적용한 결과입니다. OOS 순기대값이 양수이고 일관성·표본이 충분한 종목만 <strong>tradeable</strong>로 통과됩니다.</p>
    <div class="metrics">
      <div><span>검증 통과</span><strong class="pos">{verify.selected_count} / {verify.count}</strong></div>
      <div><span>왕복비용</span><strong>{verify.round_trip_cost_pct.toFixed(3)}%</strong></div>
      <div><span>진입 임계</span><strong>{verify.entry_threshold}</strong></div>
    </div>
    {#if verify.selected_count > 0}
      <button class="apply" on:click={applySelected}>통과 {verify.selected_count}종목을 관심종목으로 적용</button>
    {:else}
      <p class="note">통과 종목이 없습니다. 이 전략/타임프레임은 비용을 넘는 OOS 엣지가 없다는 뜻입니다.</p>
    {/if}
    <table>
      <thead><tr><th>판정</th><th>종목</th><th>코드</th><th>IS신호</th><th>IS순수익</th><th>OOS순수익</th><th>OOS일관성</th><th>OOS신호</th><th>비고</th></tr></thead>
      <tbody>
        {#each verify.items as it}
          <tr class:dim={!it.ok || !it.tradeable}>
            <td>{it.tradeable ? '✓ 통과' : '—'}</td>
            <td><strong>{it.name || '-'}</strong></td>
            <td class="cd">{it.shcode}</td>
            <td>{it.ok ? it.in_sample_signals : '-'}</td>
            <td class={it.ok && it.in_sample_avg_return >= 0 ? 'pos' : 'neg'}>{it.ok ? it.in_sample_avg_return.toFixed(2) + '%' : '-'}</td>
            <td class={it.ok && it.oos_avg_return >= 0 ? 'pos' : 'neg'}>{it.ok ? it.oos_avg_return.toFixed(2) + '%' : '-'}</td>
            <td>{it.ok ? (it.oos_consistency * 100).toFixed(0) + '%' : '-'}</td>
            <td>{it.ok ? it.oos_total_signals : '-'}</td>
            <td class="note">{it.ok ? '' : it.error}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

{#if compare}
  <div class="panel">
    <h3>전략 프리셋 비교 (walk-forward OOS)</h3>
    <table>
      <thead><tr><th>프리셋</th><th>임계</th><th>신호</th><th>승률</th><th>순수익</th><th>OOS수익</th><th>OOS일관성</th></tr></thead>
      <tbody>
        {#each compare.results as r}
          <tr><td><strong>{r.preset}</strong></td><td>{r.entry_threshold}</td><td>{r.total_signals}</td>
            <td>{(r.win_rate*100).toFixed(0)}%</td>
            <td class={r.avg_return_net>=0?'pos':'neg'}>{r.avg_return_net.toFixed(2)}%</td>
            <td class={r.oos_avg_return>=0?'pos':'neg'}>{r.oos_avg_return.toFixed(2)}%</td>
            <td>{(r.oos_consistency*100).toFixed(0)}%</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  h1 { font-size: 22px; margin: 0 0 6px; }
  .sub { color: #6c7086; font-size: 13px; margin: 0 0 16px; }
  .controls { display: flex; gap: 16px; align-items: flex-end; margin-bottom: 16px; flex-wrap: wrap; }
  .group { display: flex; flex-direction: column; gap: 6px; }
  label { font-size: 12px; color: #9399b2; }
  input, select { background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 6px 8px; }
  button { background: #89b4fa; color: #1e1e2e; border: none; border-radius: 6px; padding: 8px 18px; font-weight: 600; cursor: pointer; }
  button.alt { background: #cba6f7; }
  button.batch { background: #f9e2af; }
  button.verify { background: #a6e3a1; }
  button.apply { background: #a6e3a1; margin-bottom: 14px; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  .cd { color: #6c7086; font-size: 11px; }
  .note { color: #f9e2af; font-size: 11px; }
  tr.dim td { color: #6c7086; }
  tr.dim td strong { color: #9399b2; }
  .panel { background: #181825; border-radius: 10px; padding: 16px; margin-bottom: 16px; }
  h3 { margin: 0 0 14px; font-size: 15px; }
  .metrics { display: flex; gap: 28px; margin-bottom: 16px; flex-wrap: wrap; }
  .metrics div { display: flex; flex-direction: column; gap: 4px; }
  .metrics span { font-size: 12px; color: #9399b2; }
  .metrics strong { font-size: 20px; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th { text-align: left; padding: 7px; color: #9399b2; border-bottom: 1px solid #313244; font-weight: 500; }
  td { padding: 7px; border-bottom: 1px solid #232334; }
  .pos { color: #a6e3a1; } .neg { color: #f38ba8; }
  .error { background: #f38ba8; color: #1e1e2e; padding: 10px; border-radius: 6px; margin-bottom: 12px; }
</style>
