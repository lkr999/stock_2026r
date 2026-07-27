<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type UniverseItem, type EbestStatus, type EbestTestResult } from '$lib/api';
  import { watchlist } from '$lib/stores/watchlist';
  import { symbolStrategies } from '$lib/stores/symbolStrategies';
  import { loadDashboardState, saveDashboardState } from '$lib/stores/dashboardState';
  import { tradingStatus } from '$lib/stores/tradingStatus';
  import WatchlistPicker from '$lib/components/WatchlistPicker.svelte';

  // 이전에 저장한 대시보드 상태(설정+결과)를 복원한다 (페이지 이동 후에도 유지).
  const dsaved = loadDashboardState();

  // eBest API 통신 상태/테스트
  let ebestStatus: EbestStatus | null = null;
  let ebestResult: EbestTestResult | null = null;
  let testCode = dsaved.testCode ?? '005930';
  let testing = false;
  let apiPanelOpen = dsaved.apiPanelOpen ?? true;

  async function loadEbestStatus() {
    try { ebestStatus = await api.ebestStatus(); } catch (e) { /* ignore */ }
  }
  async function runEbestTest() {
    testing = true; ebestResult = null;
    try { ebestResult = await api.ebestTest(testCode.trim() || '005930'); }
    catch (e) { error = String(e); }
    testing = false;
    loadEbestStatus();
  }

  let error = '';

  // ── 1단계: 가격 필터 스캔 (패턴 감지 없이, 현재가/시장 필터만으로 후보 선별) ──
  let market = dsaved.market ?? 'ALL';
  let minPrice: number | undefined = dsaved.minPrice ?? 1000;
  let maxPrice: number | undefined = dsaved.maxPrice ?? 100000;
  let candidateLimit = dsaved.candidateLimit ?? 60;          // 백테스트 대상 상한 (eBest 호출 한도 고려)
  let candidates: UniverseItem[] = dsaved.candidates ?? [];
  let scanning = false;
  let scanned = dsaved.scanned ?? false;

  // ── 2단계: 후보 전체 백테스트 (모든 전략 × 모든 종목) ──
  let btTf = dsaved.btTf ?? 'auto';                // 'auto' = 전략별 권장 TF
  let btHold = dsaved.btHold ?? 25;
  let matrix: any = dsaved.matrix ?? null;
  let backtesting = false;

  // ── 3단계: OOS 기준 관심종목 선정 + 종목별 베스트 전략 배정 ──
  // 선정 기준(모두 사용자 설정) — 각 종목의 OOS 순수익 1위(거래有) 전략을 기준으로 판정한다.
  // 기본값 = 기존 강건성(tradeable) 게이트 기준: 순수익>0 · 일관성≥60% · 신호≥10.
  // 입력란 placeholder/라벨 표기에도 같은 상수를 써서 단일 출처로 관리한다.
  const OOS_DEFAULTS = { minReturn: 0, minSignals: 10, minConsistency: 60, requireTradeable: false, rankBy: 'return', maxPick: 15 };
  let oosMinReturn = dsaved.oosMinReturn ?? OOS_DEFAULTS.minReturn;                  // 최소 OOS 순수익(%)
  let oosMinSignals = dsaved.oosMinSignals ?? OOS_DEFAULTS.minSignals;              // 최소 OOS 신호수
  let oosMinConsistency = dsaved.oosMinConsistency ?? OOS_DEFAULTS.minConsistency;  // 최소 OOS 일관성(%)
  let oosRequireTradeable = dsaved.oosRequireTradeable ?? OOS_DEFAULTS.requireTradeable; // 강건성 통과(✓)만
  let oosRankBy = dsaved.oosRankBy ?? OOS_DEFAULTS.rankBy;                          // 랭킹 기준: return|consistency|signals
  let oosMaxPick = dsaved.oosMaxPick ?? OOS_DEFAULTS.maxPick;                       // 최대 선정 종목 수
  let oosSelectMsg = dsaved.oosSelectMsg ?? '';

  // 설정·결과가 바뀔 때마다 localStorage 에 저장 → 페이지 이동 후에도 복원된다.
  $: saveDashboardState({
    market, minPrice, maxPrice, candidateLimit, candidates, scanned,
    btTf, btHold, matrix, oosMinReturn, oosMinSignals, oosMinConsistency,
    oosRequireTradeable, oosRankBy, oosMaxPick, oosSelectMsg,
    testCode, apiPanelOpen,
  });

  // 1단계: 현재가/시장 필터로 후보 종목 목록만 가져온다 (패턴 스캔 아님).
  async function scan() {
    if (scanning) return;
    scanning = true; error = ''; matrix = null; oosSelectMsg = '';
    try {
      const r = await api.universe({ market, minPrice, maxPrice, limit: candidateLimit });
      candidates = r.items ?? [];
      scanned = true;
    } catch (e) {
      error = String(e);
    } finally {
      scanning = false;
    }
  }

  // 2단계: 가격 필터된 후보 전체를 모든 전략으로 OOS 백테스트.
  async function runBacktest() {
    if (backtesting) return;
    const codes = candidates.map((c) => c.code);
    if (!codes.length) { error = '먼저 ▶ 스캔으로 가격 필터된 후보를 만드세요.'; return; }
    backtesting = true; error = ''; matrix = null; oosSelectMsg = '';
    try {
      matrix = await api.strategyMatrix({ shcodes: codes, tf: btTf, max_hold_bars: btHold });
    } catch (e) {
      error = String(e);
    } finally {
      backtesting = false;
    }
  }

  // 3단계: 백테스트 결과에서 종목마다 OOS 순수익 1위(거래有) 전략을 고른 뒤,
  // 사용자가 설정한 선정 기준(최소 순수익·신호수·일관성·강건성 통과)으로 걸러
  // 랭킹 기준으로 정렬해 최대 N종목을 관심종목으로 선정하고 전략을 배정한다.
  function selectFromMatrix() {
    if (!matrix?.items?.length) { error = '먼저 ▶ 백테스트를 실행하세요.'; return; }
    type Pick = { strategy: string; tf: string; oos: number; consistency: number; signals: number; tradeable: boolean };
    // 개별 전략(셀)이 설정 기준을 통과하는지. (일관성은 % 입력을 0~1 비율과 비교)
    const passes = (it: any) =>
      it.ok &&
      (it.oos_total_signals ?? 0) > 0 &&
      (it.oos_avg_return ?? 0) >= oosMinReturn &&
      (it.oos_total_signals ?? 0) >= oosMinSignals &&
      (it.oos_consistency ?? 0) * 100 >= oosMinConsistency &&
      (!oosRequireTradeable || !!it.tradeable);
    // 종목마다 "기준을 통과하는 전략 중 OOS 순수익 1위"를 대표로 고른다.
    // → 순수익 1위 전략이 기준 미달이어도, 같은 종목에 통과하는 강건한 전략이
    //   있으면 그 전략으로 종목을 살린다(먼저 고르고 거르지 않는다).
    const bySymbol = new Map<string, Pick>();
    for (const it of matrix.items) {
      if (!passes(it)) continue;
      const cur = bySymbol.get(it.shcode);
      if (!cur || (it.oos_avg_return ?? 0) > cur.oos)
        bySymbol.set(it.shcode, {
          strategy: it.strategy, tf: it.tf,
          oos: it.oos_avg_return ?? 0,
          consistency: it.oos_consistency ?? 0,   // 0~1 비율
          signals: it.oos_total_signals ?? 0,
          tradeable: !!it.tradeable,
        });
    }
    // 랭킹 기준으로 종목 정렬 후 상위 N.
    const rankVal = (v: Pick) =>
      oosRankBy === 'consistency' ? v.consistency : oosRankBy === 'signals' ? v.signals : v.oos;
    const picks = [...bySymbol.entries()].sort((a, b) => rankVal(b[1]) - rankVal(a[1])).slice(0, oosMaxPick);
    if (!picks.length) {
      oosSelectMsg = `설정한 기준(${criteriaSummary})을 통과한 종목이 없습니다. 기준을 완화하거나 후보를 늘려보세요.`;
      return;
    }
    watchlist.clear();
    // 전략 이름과 함께 백테스트에 실제 쓰인 TF(v.tf) 도 배정에 저장 → 자동매매가
    // 백테스트와 동일한 타임프레임으로 이 전략을 돌린다.
    const map: Record<string, { strategy: string; tf: string }> = {};
    for (const [code, v] of picks) { watchlist.add(code); map[code] = { strategy: v.strategy, tf: v.tf }; }
    symbolStrategies.replace(map);
    const preview = picks.slice(0, 6).map(([c, v]) => `${c}→${v.strategy}·${v.tf}(${v.oos.toFixed(2)}%)`).join(', ');
    oosSelectMsg = `${picks.length}종목 선정 [${criteriaSummary}] — ${preview}${picks.length > 6 ? ' …' : ''}`;
  }

  // 현재 선정 기준 요약 문구 (메시지/설명에 사용).
  const rankLabel: Record<string, string> = { return: 'OOS 순수익', consistency: '일관성', signals: '신호수' };
  $: criteriaSummary = [
    `순수익≥${oosMinReturn}%`,
    oosMinSignals > 0 ? `신호≥${oosMinSignals}` : null,
    oosMinConsistency > 0 ? `일관성≥${oosMinConsistency}%` : null,
    oosRequireTradeable ? '강건성✓' : null,
    `${rankLabel[oosRankBy] ?? 'OOS 순수익'}순 상위${oosMaxPick}`,
  ].filter(Boolean).join(' · ');

  // 백테스트 결과를 종목(행) × 전략(열) 격자로 변환 — 각 셀은 OOS 순수익(%).
  type Row = { code: string; name: string; cells: Record<string, any>; best: string | null; bestOos: number };
  function buildRows(m: any): Row[] {
    if (!m?.items?.length) return [];
    const bySym = new Map<string, Row>();
    for (const it of m.items) {
      let row = bySym.get(it.shcode);
      if (!row) { row = { code: it.shcode, name: it.name, cells: {}, best: null, bestOos: -Infinity }; bySym.set(it.shcode, row); }
      row.cells[it.strategy] = it;
      // 베스트 = 실제 거래(OOS 신호>0)가 있은 전략 중 OOS 순수익 1위.
      if (it.ok && (it.oos_total_signals ?? 0) > 0 && it.oos_avg_return > row.bestOos) {
        row.bestOos = it.oos_avg_return;
        row.best = it.strategy;
      }
    }
    return [...bySym.values()].sort((a, b) => (b.bestOos === -Infinity ? -1e9 : b.bestOos) - (a.bestOos === -Infinity ? -1e9 : a.bestOos));
  }

  $: strategyCols = (matrix?.strategies ?? []) as string[];
  $: tfByStrategy = Object.fromEntries(((matrix?.by_strategy ?? []) as any[]).map((s) => [s.strategy, s.tf]));
  $: perSymbolRows = buildRows(matrix);

  // ── OOS 선정이 실제 자동매매에 반영됐는지 확인 ──
  // 선정 자체는 watchlist/symbolStrategies 스토어에 즉시 저장되지만, 그 값은
  // 자동매매 페이지에서 "불러오기 → 시작"을 눌러야 엔진에 반영된다. 이 차이를
  // 눈으로 바로 확인할 수 있도록 실행 중인 엔진의 워치리스트와 비교해 보여준다.
  $: engineWatchlist = new Set($tradingStatus.watchlist ?? []);
  $: notReflectedCodes = $watchlist.filter((c) => !engineWatchlist.has(c));

  onMount(loadEbestStatus);
</script>

<header>
  <h1>백테스트 기반 관심종목 선정 대시보드</h1>
  <p class="sub">
    ① <b>가격 필터</b>로 후보 선별 → ② 후보 전체를 <b>모든 전략으로 백테스트</b> →
    ③ <b>OOS 순수익</b> 기준 관심종목 선정 + 종목별 베스트 전략 자동 배정
  </p>
</header>

<!-- eBest API 통신 상태/테스트 -->
<div class="api-panel">
  <div class="api-head">
    <div class="api-title">
      <button class="toggle" on:click={() => (apiPanelOpen = !apiPanelOpen)}>{apiPanelOpen ? '▾' : '▸'}</button>
      <b>eBest API 통신 상태</b>
      {#if ebestStatus}
        <span class="badge {ebestStatus.token_ok ? 'live' : 'off'}">
          {ebestStatus.token_ok ? 'eBest 연결됨' : 'eBest 미연결'}
        </span>
        <span class="badge {ebestStatus.has_keys ? 'ok' : 'off'}">키 {ebestStatus.has_keys ? '설정됨' : '없음'}</span>
        <span class="badge {ebestStatus.token_ok ? 'ok' : 'off'}">토큰 {ebestStatus.token_ok ? '정상' : '없음'}</span>
      {:else}
        <span class="badge off">상태 확인중…</span>
      {/if}
    </div>
    <div class="api-actions">
      <input class="code-in" bind:value={testCode} placeholder="종목코드" />
      <button class="test-btn" on:click={runEbestTest} disabled={testing}>
        {testing ? '테스트중…' : '▶ 통신 테스트'}
      </button>
    </div>
  </div>

  {#if apiPanelOpen && ebestResult}
    <div class="api-result">
      <div class="rsum {ebestResult.ok ? 'ok' : 'fail'}">
        {ebestResult.ok ? '✓ 전체 통신 정상' : '✗ 일부 통신 실패'}
        · {ebestResult.name || ebestResult.code} ({ebestResult.code})
        · 출처 eBest 실시간
      </div>
      <table>
        <thead><tr><th>점검 항목</th><th>TR</th><th>결과</th><th>응답(ms)</th><th>상세</th></tr></thead>
        <tbody>
          {#each ebestResult.checks as ck}
            <tr>
              <td>{ck.name}</td>
              <td class="tr">{ck.tr}</td>
              <td class={ck.ok ? 'ok' : 'fail'}>{ck.ok ? '✓ 성공' : '✗ 실패'}</td>
              <td class="num">{ck.latency_ms}</td>
              <td class="detail">{ck.detail}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
  {#if apiPanelOpen && ebestStatus && !ebestStatus.token_ok}
    <p class="api-hint">⚠️ eBest 연결 안 됨 — 모든 데이터는 eBest API 에서만 가져옵니다. <code>.env</code>에 <code>EBEST_APP_KEY</code>/<code>EBEST_APP_SECRET</code>를 설정하세요. (가짜/모의 데이터는 생성하지 않습니다)</p>
  {/if}
</div>

<div class="picker-wrap">
  <WatchlistPicker {market} {minPrice} {maxPrice} />
</div>

<!-- ① 가격 필터 스캔 -->
<div class="step">
  <div class="step-head"><span class="step-no">①</span> 가격 필터로 후보 선별 <span class="step-sub">— 패턴 감지 없이 현재가/시장 필터만 적용</span></div>
  <div class="controls">
    <div class="group">
      <label>시장</label>
      <select bind:value={market}><option>ALL</option><option>KOSPI</option><option>KOSDAQ</option></select>
    </div>
    <div class="group"><label>현재가 최소</label><input type="number" bind:value={minPrice} step="500" min="0" /></div>
    <div class="group"><label>현재가 최대</label><input type="number" bind:value={maxPrice} step="500" min="0" /></div>
    <div class="group"><label>후보 상한(종목수)</label><input type="number" bind:value={candidateLimit} step="10" min="1" /></div>
    <button class="refresh" on:click={scan} disabled={scanning}>
      {scanning ? '스캔중…' : scanned ? '↻ 다시 스캔' : '▶ 스캔'}
    </button>
    {#if scanned}<span class="cand-count">후보 <b>{candidates.length}</b>종목</span>{/if}
    <a class="to-trading" href="/trading">관심종목으로 자동매매 →</a>
  </div>
</div>

<!-- ② 후보 전체 백테스트 -->
<div class="step">
  <div class="step-head"><span class="step-no">②</span> 후보 전체 백테스트 <span class="step-sub">— 종목 × 모든 전략 OOS 검증</span></div>
  <div class="controls">
    <div class="group">
      <label>타임프레임</label>
      <select bind:value={btTf}>
        <option value="auto">auto (전략별 권장 TF)</option>
        {#each ['1m','3m','5m','10m','15m','30m','60m','1d'] as t}<option value={t}>{t}</option>{/each}
      </select>
    </div>
    <div class="group"><label>보유봉수</label>
      <select bind:value={btHold}>{#each [5,10,25,40,60] as h}<option value={h}>{h}</option>{/each}</select></div>
    <button class="bt-btn" on:click={runBacktest} disabled={backtesting || !candidates.length}>
      {backtesting ? '백테스트 중…' : `▶ 후보 ${candidates.length}종목 백테스트`}
    </button>
    <span class="bt-note">auto는 종목당 1m·5m를 모두 조회하므로 후보 수에 비례해 시간이 걸립니다(종목당 약 1초/봉).</span>
  </div>
</div>

<!-- ③ OOS 기준 선정 (선정 기준 사용자 설정) -->
<div class="step">
  <div class="step-head"><span class="step-no">③</span> 관심종목 선정 기준 설정 <span class="step-sub">— 종목별 베스트 전략 자동 배정</span></div>
  <div class="controls">
    <div class="group"><label>최소 OOS 순수익(%) <span class="dflt">기본 {OOS_DEFAULTS.minReturn}</span></label><input type="number" bind:value={oosMinReturn} step="0.05" placeholder={String(OOS_DEFAULTS.minReturn)} /></div>
    <div class="group"><label>최소 OOS 신호수 <span class="dflt">기본 {OOS_DEFAULTS.minSignals}</span></label><input type="number" bind:value={oosMinSignals} step="1" min="0" placeholder={String(OOS_DEFAULTS.minSignals)} /></div>
    <div class="group"><label>최소 일관성(%) <span class="dflt">기본 {OOS_DEFAULTS.minConsistency}</span></label><input type="number" bind:value={oosMinConsistency} step="5" min="0" max="100" placeholder={String(OOS_DEFAULTS.minConsistency)} /></div>
    <div class="group"><label>최대 선정 종목수 <span class="dflt">기본 {OOS_DEFAULTS.maxPick}</span></label><input type="number" bind:value={oosMaxPick} step="1" min="1" placeholder={String(OOS_DEFAULTS.maxPick)} /></div>
    <div class="group">
      <label>랭킹 기준 <span class="dflt">기본 OOS 순수익</span></label>
      <select bind:value={oosRankBy}>
        <option value="return">OOS 순수익</option>
        <option value="consistency">일관성</option>
        <option value="signals">신호수</option>
      </select>
    </div>
    <label class="chk" title="OOS 순수익>0 · 일관성≥60% · 신호≥10 을 모두 통과(✓)한 종목만 선정">
      <input type="checkbox" bind:checked={oosRequireTradeable} /> 강건성 통과(✓)만 <span class="dflt">기본 해제</span>
    </label>
    <button class="bt-btn pick" on:click={selectFromMatrix} disabled={!matrix}>★ 기준으로 선정</button>
  </div>
  <div class="bt-note">현재 기준: <b>{criteriaSummary}</b> — 종목마다 OOS 순수익 1위(거래有) 전략을 골라 위 기준으로 걸러 관심종목으로 선정하고, 자동매매에 종목별로 다르게 적용합니다.</div>
  {#if oosSelectMsg}<div class="hint added">★ {oosSelectMsg}</div>{/if}
  {#if Object.keys($symbolStrategies).length}
    <div class="bt-assign">배정됨:
      {#each $watchlist.filter((c) => $symbolStrategies[c]) as c}
        <span class="assign-chip">{c} → <b>{$symbolStrategies[c].strategy}</b>{#if $symbolStrategies[c].tf}<em>{$symbolStrategies[c].tf}</em>{/if}</span>
      {/each}
    </div>
  {/if}
  {#if $watchlist.length}
    <div class="reflect {!$tradingStatus.running ? 'off' : notReflectedCodes.length ? 'warn' : 'ok'}">
      {#if !$tradingStatus.running}
        ⚪ 자동매매가 <b>정지</b> 상태입니다 — 선정 결과는 저장되었지만,
        <a href="/trading">자동매매 페이지</a>에서 <b>불러오기 → 시작</b>을 눌러야 실제로 반영됩니다.
      {:else if notReflectedCodes.length === 0}
        🟢 실행 중인 자동매매({$tradingStatus.mode === 'live' ? '실전' : '모의'})에 선정한 {$watchlist.length}종목이 모두 반영되어 있습니다.
      {:else}
        🟡 실행 중인 자동매매에 <b>{notReflectedCodes.length}종목이 아직 반영되지 않았습니다</b>: {notReflectedCodes.join(', ')} —
        <a href="/trading">자동매매 페이지</a>에서 불러오기 후 다시 시작하세요.
      {/if}
    </div>
  {/if}
</div>

{#if error}<div class="error">{error}</div>{/if}

<!-- 종목별 × 전략별 백테스트 결과 (OOS 순수익) -->
{#if matrix}
  <div class="panel">
    <h3>종목별 · 전략별 백테스트 결과 — OOS 순수익(%) · {matrix.auto ? 'TF auto(전략별 권장)' : matrix.timeframe} · 보유 {matrix.max_hold_bars}봉</h3>
    <p class="legend">셀 = OOS 순수익(%) · <b>굵게</b> = 그 종목 OOS 순수익 1위(거래有) 전략 = 선정 기준 · <span class="tick">✓</span> = 강건성 통과(순수익&gt;0·일관성≥60%·신호≥10, 참고용)</p>
    <div class="grid-wrap">
      <table class="grid">
        <thead>
          <tr>
            <th class="sticky">종목</th>
            {#each strategyCols as s}<th>{s}<span class="th-tf">{tfByStrategy[s] ?? ''}</span></th>{/each}
            <th>베스트</th><th>★</th>
          </tr>
        </thead>
        <tbody>
          {#each perSymbolRows as row}
            <tr class:watched={$watchlist.includes(row.code)}>
              <td class="sticky"><strong>{row.name || row.code}</strong><span class="code">{row.code}</span></td>
              {#each strategyCols as s}
                {@const it = row.cells[s]}
                <td class="cell">
                  {#if it && it.ok}
                    <span class={(it.oos_avg_return ?? 0) >= 0 ? 'pos' : 'neg'} class:bestcell={row.best === s}>
                      {(it.oos_avg_return ?? 0).toFixed(2)}{it.tradeable ? ' ✓' : ''}
                    </span>
                  {:else}<span class="na">-</span>{/if}
                </td>
              {/each}
              <td>{row.best ? row.best : '—'}</td>
              <td>
                <button class="star" class:on={$watchlist.includes(row.code)}
                  on:click={() => { watchlist.toggle(row.code); if (row.best) symbolStrategies.setOne(row.code, { strategy: row.best, tf: row.cells[row.best]?.tf }); }}
                  title="관심종목 추가/제거 (추가 시 베스트 전략 배정)">{$watchlist.includes(row.code) ? '★' : '☆'}</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>
{:else if scanned}
  <div class="panel empty">가격 필터로 후보 {candidates.length}종목을 선별했습니다. <b>② 백테스트</b>를 실행하면 종목별·전략별 OOS 순수익이 표시됩니다.</div>
{/if}

<style>
  header h1 { margin: 0 0 4px; font-size: 22px; }
  .sub { color: #a6adc8; margin: 0 0 16px; font-size: 13px; }
  .sub code { background: #313244; padding: 1px 5px; border-radius: 4px; color: #bac2de; }
  .picker-wrap { margin-bottom: 14px; }
  .controls { display: flex; gap: 16px; align-items: flex-end; margin-bottom: 16px; flex-wrap: wrap; }
  .group { display: flex; flex-direction: column; gap: 6px; }
  label { font-size: 12px; color: #bac2de; }
  select, input[type='number'] { background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 5px 8px; width: 110px; }
  .refresh { background: #89b4fa; color: #1e1e2e; border: none; border-radius: 6px; padding: 8px 16px; font-weight: 600; cursor: pointer; }
  .hint { font-size: 13px; color: #f9e2af; background: #1f1d2e; border: 1px solid #45475a; border-radius: 6px; padding: 8px 12px; margin: 10px 0 0; }
  .hint.added { color: #a6e3a1; }
  .step { background: #181825; border: 1px solid #313244; border-radius: 10px; padding: 12px 14px; margin-bottom: 12px; }
  .step-head { font-size: 14px; color: #cdd6f4; margin-bottom: 10px; }
  .step-no { color: #89b4fa; font-weight: 700; }
  .step-sub { color: #a6adc8; font-size: 12px; font-weight: 400; }
  .step .controls { margin-bottom: 0; }
  .cand-count { font-size: 13px; color: #cdd6f4; align-self: center; }
  .cand-count b { color: #a6e3a1; }
  .bt-btn { background: #cba6f7; color: #1e1e2e; border: none; border-radius: 6px; padding: 8px 16px; font-weight: 600; cursor: pointer; }
  .bt-btn.pick { background: #a6e3a1; }
  .bt-btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .chk { display: flex; align-items: center; gap: 6px; font-size: 13px; color: #cdd6f4; align-self: center; cursor: pointer; white-space: nowrap; }
  .chk input { width: auto; }
  .dflt { color: #a6adc8; font-size: 11px; font-weight: 400; }
  .bt-note { font-size: 12px; color: #a6adc8; flex: 1; min-width: 240px; line-height: 1.5; }
  .bt-assign { margin-top: 10px; display: flex; gap: 8px; flex-wrap: wrap; font-size: 12px; color: #bac2de; }
  .assign-chip { background: #1e1e2e; border: 1px solid #313244; border-radius: 10px; padding: 2px 8px; }
  .assign-chip b { color: #cba6f7; }
  .assign-chip em { color: #a6adc8; font-style: normal; font-size: 11px; margin-left: 4px; }
  .reflect { margin-top: 10px; font-size: 12px; padding: 8px 12px; border-radius: 6px; border: 1px solid #45475a; }
  .reflect a { color: inherit; text-decoration: underline; }
  .reflect.off { color: #bac2de; background: #1e1e2e; }
  .reflect.ok { color: #a6e3a1; background: #1a2a1a; border-color: #3a4a3a; }
  .reflect.warn { color: #f9e2af; background: #2a2717; border-color: #4a4530; }
  .to-trading { margin-left: auto; color: #a6e3a1; text-decoration: none; font-size: 13px; align-self: center; }
  .panel { background: #181825; border-radius: 10px; padding: 12px 16px; margin-top: 4px; }
  .panel.empty { color: #bac2de; font-size: 13px; }
  .panel h3 { margin: 0 0 6px; font-size: 15px; }
  .legend { color: #a6adc8; font-size: 12px; margin: 0 0 12px; }
  .error { background: #f38ba8; color: #1e1e2e; padding: 10px; border-radius: 6px; margin-bottom: 12px; font-size: 13px; }
  /* 종목 × 전략 OOS 격자 */
  .grid-wrap { overflow-x: auto; }
  table.grid { width: 100%; border-collapse: collapse; font-size: 12px; }
  table.grid th { text-align: right; padding: 6px 8px; color: #bac2de; border-bottom: 1px solid #313244; font-weight: 500; white-space: nowrap; }
  table.grid th:first-child { text-align: left; }
  table.grid td { padding: 6px 8px; border-bottom: 1px solid #232334; text-align: right; font-variant-numeric: tabular-nums; }
  table.grid td:first-child { text-align: left; }
  .th-tf { display: block; font-size: 10px; color: #a6adc8; font-weight: 400; }
  .sticky { position: sticky; left: 0; background: #181825; z-index: 1; }
  tr.watched .sticky { background: #1a1a2a; }
  .code { color: #a6adc8; margin-left: 6px; font-size: 11px; }
  .cell .bestcell { font-weight: 700; background: #1d2a1d; padding: 1px 5px; border-radius: 4px; }
  .na { color: #45475a; }
  .pos { color: #a6e3a1; } .neg { color: #f38ba8; }
  .tick { color: #a6e3a1; }
  .star { background: none; border: none; cursor: pointer; font-size: 15px; color: #a6adc8; padding: 0; }
  .star.on { color: #f9e2af; }

  /* eBest API 패널 */
  .api-panel { background: #181825; border: 1px solid #313244; border-radius: 10px; padding: 12px 14px; margin-bottom: 14px; }
  .api-head { display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap; }
  .api-title { display: flex; align-items: center; gap: 8px; font-size: 14px; flex-wrap: wrap; }
  .toggle { background: none; border: none; color: #bac2de; cursor: pointer; font-size: 13px; padding: 0; }
  .badge { font-size: 11px; font-weight: 700; padding: 2px 8px; border-radius: 10px; }
  .badge.mock { background: #f9e2af; color: #1e1e2e; }
  .badge.live { background: #a6e3a1; color: #1e1e2e; }
  .badge.ok { background: #313244; color: #a6e3a1; }
  .badge.off { background: #313244; color: #f38ba8; }
  .api-actions { display: flex; gap: 8px; align-items: center; }
  .code-in { background: #1e1e2e; color: #cdd6f4; border: 1px solid #45475a; border-radius: 4px; padding: 5px 8px; width: 100px; }
  .test-btn { background: #cba6f7; color: #1e1e2e; border: none; border-radius: 6px; padding: 7px 14px; font-weight: 700; cursor: pointer; }
  .test-btn:disabled { opacity: 0.5; cursor: progress; }
  .api-result { margin-top: 12px; }
  .rsum { font-size: 13px; margin-bottom: 8px; font-weight: 600; }
  .rsum.ok { color: #a6e3a1; }
  .rsum.fail { color: #f38ba8; }
  .api-result table { width: 100%; border-collapse: collapse; font-size: 12px; }
  .api-result th { text-align: left; padding: 5px 7px; color: #bac2de; border-bottom: 1px solid #313244; font-weight: 500; }
  .api-result td { padding: 5px 7px; border-bottom: 1px solid #232334; }
  .api-result td.ok { color: #a6e3a1; }
  .api-result td.fail { color: #f38ba8; }
  .api-result td.tr { color: #a6adc8; font-family: monospace; }
  .api-result td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .api-result td.detail { color: #bac2de; }
  .api-hint { font-size: 12px; color: #a6adc8; margin: 10px 0 0; }
  .api-hint code { background: #313244; padding: 1px 5px; border-radius: 4px; color: #bac2de; }
</style>
