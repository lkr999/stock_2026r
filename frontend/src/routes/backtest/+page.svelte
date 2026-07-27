<script lang="ts">
  import { get } from 'svelte/store';
  import { api } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';

  let tf = 'auto';   // 'auto' = 전략마다 권장 타임프레임으로 백테스트
  let holdBars = 25;
  let matrix: any = null;       // 트레이더 × 종목 검증
  let batch: any = null;        // 패턴별 통계(참고)
  let filterStrategy = 'all';   // 매트릭스 표 트레이더 필터
  let loading = false;
  let error = '';
  let showHelp = true;

  // 관심종목 전체 × 전략(트레이더) 매트릭스 검증 — 기본 동작
  async function runMatrix() {
    const shcodes = get(watchlist);
    if (!shcodes.length) { error = '관심종목이 비어 있습니다. 대시보드에서 자동매매 종목을 먼저 선택하세요.'; return; }
    loading = true; error = ''; matrix = null; batch = null;
    try {
      matrix = await api.strategyMatrix({ shcodes, tf, max_hold_bars: holdBars });
      filterStrategy = matrix?.best_strategy ?? 'all';
    } catch (e) { error = String(e); }
    loading = false;
  }

  // 관심종목 패턴별 통계 (참고용)
  async function runBatch() {
    const shcodes = get(watchlist);
    if (!shcodes.length) { error = '관심종목이 비어 있습니다. 대시보드에서 자동매매 종목을 먼저 선택하세요.'; return; }
    loading = true; error = ''; batch = null; matrix = null;
    try {
      batch = await api.batchBacktest({ shcodes, tf, hold_bars: holdBars });
    } catch (e) { error = String(e); }
    loading = false;
  }

  // 선택한 트레이더(전략)의 검증 통과 종목만 관심종목으로 교체
  function applySelected(stratName: string) {
    const row = (matrix?.by_strategy ?? []).find((s: any) => s.strategy === stratName);
    if (!row?.selected?.length) return;
    watchlist.clear();
    for (const code of row.selected) watchlist.add(code);
  }

  const pct = (v: number, d = 2) => (v * 100).toFixed(d);
  const bestPattern = (it: any) =>
    (it.by_pattern || []).filter((b: any) => b.signals > 0).sort((a: any, b: any) => b.avg_return - a.avg_return)[0];
  $: matrixItems = matrix
    ? (filterStrategy === 'all' ? matrix.items : matrix.items.filter((it: any) => it.strategy === filterStrategy))
    : [];
</script>

<h1>백테스트 — 관심종목 전체 검증</h1>
<p class="sub">대시보드에서 선택한 <strong>자동매매 관심종목 전체</strong>를 대상으로, 모든 전략 프리셋(트레이더)을 종목별로 검증하고 실제 베스트 전략을 비교합니다. 반전 패턴 전략과 <strong>단타 셋업(VWAP·시가돌파·EMA눌림목, 롱+숏)</strong>이 함께 포함됩니다. (왕복 거래비용 차감)</p>

<div class="help">
  <button class="help-toggle" on:click={() => showHelp = !showHelp}>
    {showHelp ? '▾' : '▸'} 트레이딩 방식 · 보유봉수 설명
  </button>
  {#if showHelp}
    <div class="help-body">
      <h4>트레이딩 방식 (실매매와 동일 조건으로 검증)</h4>
      <ul>
        <li><b>진입</b>: 캔들패턴·거래량·ATR 등 활성 소스의 <b>종합점수</b>가 전략의 <b>진입 임계값</b> 이상이고, 확인봉·상위TF 추세 게이트를 통과하면 <b>다음 봉 시가</b>에 매수합니다.</li>
        <li><b>청산</b>: ATR 기반 <b>손절 / 익절 / 트레일링 스톱</b> 중 먼저 닿는 선에서 청산하고, 어디에도 닿지 않으면 <b>보유봉수</b> 도달 시 시간청산합니다.</li>
        <li><b>거래비용</b>: 매수·매도 왕복 수수료 + 세금 + 슬리피지({matrix?.round_trip_cost_pct?.toFixed(3) ?? batch?.round_trip_cost_pct?.toFixed(3) ?? '0.230'}%)를 차감한 <b>순수익</b>으로 평가합니다.</li>
        <li><b>OOS 검증</b>: 데이터를 walk-forward로 4분할해 <b>학습에 쓰지 않은 구간(out-of-sample)</b>의 성과만 집계합니다. 과최적화를 배제한 실전 기대값입니다.</li>
        <li><b>tradeable(통과) 판정</b>: OOS 순수익 &gt; 0, OOS 일관성 ≥ 60%, OOS 표본 ≥ 10건을 모두 만족해야 합니다.</li>
        <li><b>반전 트레이더</b>: <code>conservative</code>(보수·높은 임계) · <code>balanced</code>(균형) · <code>aggressive</code>(공격·낮은 임계) · <code>ml_blended</code>(ML 혼합)는 각각 점수 가중치·진입 임계·사용 패턴이 다릅니다.</li>
        <li><b>단타 트레이더 (롱+숏)</b>: <code>vwap_scalp</code>(VWAP 되찾기/되돌림) · <code>orb_breakout</code>(시가 레인지 돌파) · <code>ema_pullback</code>(EMA 눌림목 연속) · <code>intraday_blended</code>(혼합). 모두 <b>1분봉</b> 기준 세션 VWAP·개장 레인지·EMA 컨텍스트로 진입하며, 하락 셋업은 <b>숏(페이퍼 시뮬레이션)</b>으로 진입합니다.</li>
        <li><b>타임프레임 <code>auto</code> (기본)</b>: 각 트레이더를 <b>자신의 권장 타임프레임</b>(반전 전략 5m · 단타 전략 1m)으로 검증합니다. 반전·단타 전략을 한 화면에서 <b>공정하게</b> 비교하려면 auto를 쓰고, 특정 봉으로 통일해 보려면 봉을 직접 선택하세요. (auto는 종목당 1m·5m 둘 다 조회하므로 시간이 더 걸립니다.)</li>
      </ul>
      <h4>보유봉수란?</h4>
      <p>
        매수 후 손절·익절·트레일링에 닿지 않아도 <b>최대로 보유하는 봉(캔들) 개수</b>입니다. 이 봉수에 도달하면 손익과 무관하게 <b>시간청산</b>합니다.
        실제 보유 시간은 <b>타임프레임 × 보유봉수</b>입니다 — 예: <code>5m</code> · 보유봉수 <code>25</code> ≈ 최대 125분 보유 후 청산.
        <b>짧을수록</b> 회전이 빠르고 자금이 덜 묶이지만 추세 수익을 일찍 끊을 수 있고, <b>길수록</b> 추세 수익을 더 담지만 자금이 오래 묶이고 역추세 위험이 커집니다.
      </p>
    </div>
  {/if}
</div>

<div class="controls">
  <div class="group"><label>타임프레임</label>
    <select bind:value={tf}>
      <option value="auto">auto (전략별 권장 TF)</option>
      {#each ['1m','3m','5m','10m','15m','30m','60m','1d'] as t}<option value={t}>{t}</option>{/each}
    </select></div>
  <div class="group"><label title="매수 후 최대 보유 봉수 (도달 시 시간청산)">보유봉수</label>
    <select bind:value={holdBars}>{#each [5,10,25,40,60] as h}<option value={h}>{h}</option>{/each}</select></div>
  <button class="verify" on:click={runMatrix} disabled={loading} title="관심종목 전체를 모든 트레이더(전략)로 종목별 OOS 검증하고 베스트 전략을 비교">
    ★ 관심종목 전체 검증 ({$watchlist.length})
  </button>
  <button class="alt" on:click={runBatch} disabled={loading} title="관심종목의 캔들패턴별 통계(참고용)">
    패턴별 통계 (참고)
  </button>
</div>

{#if error}<div class="error">{error}</div>{/if}
{#if loading}<p>계산중… (트레이더 × 종목 검증은 종목 수에 비례해 시간이 걸립니다)</p>{/if}

{#if matrix}
  <!-- 베스트 전략 배너 -->
  {#if matrix.best_strategy}
    {@const best = matrix.by_strategy[0]}
    <div class="best">
      <div class="best-head">🏆 베스트 트레이더: <strong>{matrix.best_strategy}</strong></div>
      <div class="best-stats">
        통과 종목 <strong>{best.tradeable_count}/{matrix.count}</strong> ·
        OOS 순수익(가중) <strong class={best.oos_avg_return >= 0 ? 'pos' : 'neg'}>{best.oos_avg_return.toFixed(2)}%</strong> ·
        OOS 일관성 <strong>{pct(best.oos_consistency, 0)}%</strong> ·
        진입 임계 <strong>{best.entry_threshold}</strong>
      </div>
      {#if best.tradeable_count > 0}
        <button class="apply" on:click={() => applySelected(matrix.best_strategy)}>이 트레이더의 통과 {best.tradeable_count}종목을 관심종목으로 적용</button>
      {/if}
    </div>
  {/if}

  <!-- 전략(트레이더) 비교 -->
  <div class="panel">
    <h3>전략(트레이더) 비교 — {matrix.count}종목 · {matrix.auto ? 'TF auto(전략별 권장)' : matrix.timeframe} · 보유 {matrix.max_hold_bars}봉 (best 순)</h3>
    <table>
      <thead><tr><th>순위</th><th>트레이더</th><th>방향</th><th>TF</th><th>진입임계</th><th>통과 종목</th><th>OOS 순수익(가중)</th><th>OOS 일관성</th><th>OOS 신호</th><th>적용</th></tr></thead>
      <tbody>
        {#each matrix.by_strategy as s, i}
          <tr class:winner={s.strategy === matrix.best_strategy}>
            <td>{i === 0 ? '🏆 1' : i + 1}</td>
            <td><strong>{s.strategy}</strong></td>
            <td>{#if s.direction === 'both'}<span class="badge short">롱+숏</span>{:else}<span class="badge long">롱</span>{/if}</td>
            <td class="cd">{s.tf ?? matrix.timeframe}</td>
            <td>{s.entry_threshold}</td>
            <td class={s.tradeable_count > 0 ? 'pos' : ''}>{s.tradeable_count} / {matrix.count}</td>
            <td class={s.oos_avg_return >= 0 ? 'pos' : 'neg'}>{s.oos_avg_return.toFixed(2)}%</td>
            <td>{pct(s.oos_consistency, 0)}%</td>
            <td>{s.oos_total_signals}</td>
            <td>{#if s.tradeable_count > 0}<button class="apply sm" on:click={() => applySelected(s.strategy)}>적용</button>{:else}-{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>

  <!-- 트레이더 × 종목 매트릭스 -->
  <div class="panel">
    <div class="panel-head">
      <h3>트레이더별 · 종목별 검증</h3>
      <div class="group inline">
        <label>트레이더 필터</label>
        <select bind:value={filterStrategy}>
          <option value="all">전체</option>
          {#each matrix.strategies as s}<option value={s}>{s}</option>{/each}
        </select>
      </div>
    </div>
    <table>
      <thead><tr><th>판정</th><th>트레이더</th><th>TF</th><th>종목</th><th>코드</th><th>IS신호</th><th>IS순수익</th><th>IS승률</th><th>OOS순수익</th><th>OOS일관성</th><th>OOS신호</th><th>비고</th></tr></thead>
      <tbody>
        {#each matrixItems as it}
          <tr class:dim={!it.ok || !it.tradeable}>
            <td>{it.tradeable ? '✓ 통과' : '—'}</td>
            <td>{it.strategy}</td>
            <td class="cd">{it.tf ?? matrix.timeframe}</td>
            <td><strong>{it.name || '-'}</strong></td>
            <td class="cd">{it.shcode}</td>
            <td>{it.ok ? it.in_sample_signals : '-'}</td>
            <td class={it.ok && it.in_sample_avg_return >= 0 ? 'pos' : 'neg'}>{it.ok ? it.in_sample_avg_return.toFixed(2) + '%' : '-'}</td>
            <td>{it.ok ? (it.in_sample_win_rate * 100).toFixed(0) + '%' : '-'}</td>
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

{#if batch}
  <div class="panel">
    <h3>패턴별 통계 (참고) — {batch.count}종목 ({batch.timeframe}, {batch.hold_bars}봉)</h3>
    <div class="metrics">
      <div><span>대상 종목</span><strong>{batch.count}</strong></div>
      <div><span>신호 발생 종목</span><strong>{batch.graded_count}</strong></div>
      <div><span>총 신호</span><strong>{batch.aggregate.total_signals}</strong></div>
      <div><span>가중 승률</span><strong>{(batch.aggregate.win_rate * 100).toFixed(1)}%</strong></div>
      <div><span>가중 평균순수익</span><strong class={batch.aggregate.avg_return >= 0 ? 'pos' : 'neg'}>{batch.aggregate.avg_return.toFixed(3)}%</strong></div>
    </div>
    <table>
      <thead><tr><th>종목</th><th>코드</th><th>신호</th><th>승률</th><th>평균순수익</th><th>최고 패턴</th><th>비고</th></tr></thead>
      <tbody>
        {#each batch.items as it}
          {@const best = bestPattern(it)}
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

<style>
  h1 { font-size: 22px; margin: 0 0 6px; }
  .sub { color: #a6adc8; font-size: 13px; margin: 0 0 16px; }
  .help { background: #181825; border-radius: 10px; margin-bottom: 16px; }
  .help-toggle { background: none; border: none; color: #89b4fa; font-size: 13px; font-weight: 600; cursor: pointer; padding: 12px 16px; width: 100%; text-align: left; }
  .help-body { padding: 0 18px 16px; color: #bac2de; font-size: 13px; line-height: 1.6; }
  .help-body h4 { color: #cdd6f4; font-size: 13px; margin: 10px 0 6px; }
  .help-body ul { margin: 0; padding-left: 18px; }
  .help-body li { margin-bottom: 4px; }
  .help-body code { background: #313244; padding: 1px 6px; border-radius: 4px; font-size: 12px; }
  .controls { display: flex; gap: 16px; align-items: flex-end; margin-bottom: 16px; flex-wrap: wrap; }
  .group { display: flex; flex-direction: column; gap: 6px; }
  .group.inline { flex-direction: row; align-items: center; gap: 8px; }
  label { font-size: 12px; color: #bac2de; }
  select { background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 6px 8px; }
  button { background: #89b4fa; color: #1e1e2e; border: none; border-radius: 6px; padding: 8px 18px; font-weight: 600; cursor: pointer; }
  button.alt { background: #cba6f7; }
  button.verify { background: #a6e3a1; }
  button.apply { background: #a6e3a1; margin-top: 10px; }
  button.apply.sm { padding: 3px 12px; font-size: 12px; margin: 0; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  .cd { color: #a6adc8; font-size: 11px; }
  .note { color: #f9e2af; font-size: 11px; }
  tr.dim td { color: #a6adc8; }
  tr.dim td strong { color: #bac2de; }
  tr.winner td { background: #1d2a1d; }
  .best { background: linear-gradient(90deg, #1d2a1d, #181825); border: 1px solid #2a3a2a; border-radius: 10px; padding: 14px 18px; margin-bottom: 16px; }
  .best-head { font-size: 16px; margin-bottom: 6px; }
  .best-head strong { color: #a6e3a1; }
  .best-stats { font-size: 13px; color: #bac2de; }
  .panel { background: #181825; border-radius: 10px; padding: 16px; margin-bottom: 16px; }
  .panel-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 14px; }
  .panel-head h3 { margin: 0; }
  h3 { margin: 0 0 14px; font-size: 15px; }
  h4 { color: #bac2de; }
  .metrics { display: flex; gap: 28px; margin-bottom: 16px; flex-wrap: wrap; }
  .metrics div { display: flex; flex-direction: column; gap: 4px; }
  .metrics span { font-size: 12px; color: #bac2de; }
  .metrics strong { font-size: 20px; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th { text-align: left; padding: 7px; color: #bac2de; border-bottom: 1px solid #313244; font-weight: 500; }
  td { padding: 7px; border-bottom: 1px solid #232334; }
  .pos { color: #a6e3a1; } .neg { color: #f38ba8; }
  .badge { font-size: 11px; padding: 1px 7px; border-radius: 10px; font-weight: 600; }
  .badge.long { background: #1d2a1d; color: #a6e3a1; }
  .badge.short { background: #2a1d2a; color: #f5c2e7; }
  .error { background: #f38ba8; color: #1e1e2e; padding: 10px; border-radius: 6px; margin-bottom: 12px; }
</style>
