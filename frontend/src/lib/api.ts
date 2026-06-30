export type Timeframe = '1m' | '3m' | '5m' | '10m' | '15m' | '30m' | '60m' | '1d';

export interface Candle {
  ts: string;
  date?: string;
  time?: string;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

export interface PatternResult {
  pattern_name: string;
  pattern_type: 'bullish' | 'bearish';
  detected_at: string;
  confidence: number;
  composite_score: number;
  atr_normalized: boolean;
  volume_confirmed: boolean;
  mtf_score: number;
  ml_score: number;
  expected_5: number;
  expected_10: number;
  expected_25: number;
  candles_used: Candle[];
}

export interface SignalItem {
  shcode: string;
  name: string;
  market?: string;
  price?: number;
  change_pct?: number;
  pattern_name: string;
  pattern_type: 'bullish' | 'bearish';
  confidence: number;
  composite_score: number;
  atr_normalized?: boolean;
  volume_confirmed?: boolean;
  detected_at: string;
  expected_5?: number;
}

export interface UniverseItem {
  code: string;
  name: string;
  market: string;
  price: number;
  prev_close: number;
  change_pct: number;
  volume: number;
}

export interface Indicators {
  ma5: (number | null)[];
  ma20: (number | null)[];
  ma60: (number | null)[];
  bb_upper: (number | null)[];
  bb_mid: (number | null)[];
  bb_lower: (number | null)[];
  rsi14: (number | null)[];
  macd: (number | null)[];
  macd_signal: (number | null)[];
  macd_hist: (number | null)[];
  // 단타 오버레이: 세션 VWAP + 개장 레인지 박스 + 단기 EMA
  vwap: (number | null)[];
  or_high: (number | null)[];
  or_low: (number | null)[];
  ema9: (number | null)[];
  ema20: (number | null)[];
}

export interface IndicatorsResponse {
  shcode: string;
  name: string;
  timeframe: string;
  candles: Candle[];
  indicators: Indicators;
  data_source: 'live';
}

/** 거래 이벤트 (엔진 인메모리, 차트 마커 + 거래내역 표시용). */
export interface TradeEvent {
  code: string;
  name: string;
  type: 'buy' | 'sell';
  side?: 'long' | 'short';   // 롱/숏 (숏은 페이퍼 시뮬레이션)
  action?: 'open' | 'close'; // 진입/청산
  price: number;
  qty: number;
  pnl: number;
  pnl_pct: number;
  reason: string;
  ts: number;         // UTC Unix 초 (차트 타임스케일과 동일)
  time_label: string;
}

export interface Quote {
  shcode: string;
  name: string;
  price: number;
  prev_close: number;
  change: number;
  change_pct: number;
  date: string;
  time: string;
  data_source: 'live';
}

export interface TradeStats {
  count: number;
  win_count: number;
  loss_count: number;
  total_pnl: number;
  win_rate: number;
  profit_factor: number | null;
  avg_return_pct: number;
  best_trade_pnl: number;
  worst_trade_pnl: number;
}

const BASE = '';

async function jget<T>(url: string): Promise<T> {
  const r = await fetch(`${BASE}${url}`);
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json();
}
async function jpost<T>(url: string, body: unknown): Promise<T> {
  const r = await fetch(`${BASE}${url}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json();
}
async function jput<T>(url: string, body: unknown): Promise<T> {
  const r = await fetch(`${BASE}${url}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json();
}

export interface EbestStatus {
  has_keys: boolean;
  token_ok: boolean | null;
  base_url: string;
}
export interface EbestCheck {
  name: string;
  tr: string;
  ok: boolean;
  latency_ms: number;
  detail: string;
}
export interface EbestTestResult {
  ok: boolean;
  source: 'ebest';
  code: string;
  name: string;
  checks: EbestCheck[];
}

export const api = {
  health: () => jget<{ ok: boolean; ebest_configured: boolean }>(`/api/health`),
  ebestStatus: () => jget<EbestStatus>(`/api/ebest/status`),
  ebestTest: (code = '005930') => jpost<EbestTestResult>(`/api/ebest/test?code=${code}`, {}),
  candles: (code: string, tf: Timeframe) =>
    jget<{ candles: Candle[]; name?: string; data_source: 'live' }>(`/api/candles/${code}?tf=${tf}`),
  indicators: (code: string, tf: Timeframe) =>
    jget<IndicatorsResponse>(`/api/candles/${code}/indicators?tf=${tf}`),
  quote: (code: string, tf: Timeframe = '1m') =>
    jget<Quote>(`/api/candles/${code}/quote?tf=${tf}`),
  lookup: (codes: string[]) =>
    jget<{ code: string; name: string; market: string; price: number }[]>(
      `/api/universe/lookup?codes=${codes.join(',')}`
    ),
  patterns: (code: string, tf: Timeframe, strategy: string, mtf = false) =>
    jget<{ name: string; patterns: PatternResult[] }>(
      `/api/patterns/${code}?tf=${tf}&strategy=${strategy}&mtf=${mtf}`
    ),
  signals: (
    market: string,
    tf: Timeframe,
    strategy: string,
    minConf = 0.6,
    scanLimit = 120,
    minPrice?: number,
    maxPrice?: number
  ) => {
    let url = `/api/signals?market=${market}&tf=${tf}&strategy=${strategy}&min_conf=${minConf}&scan_limit=${scanLimit}`;
    if (minPrice != null) url += `&min_price=${minPrice}`;
    if (maxPrice != null) url += `&max_price=${maxPrice}`;
    return jget<SignalItem[]>(url);
  },
  watchlistSignals: (codes: string[], tf: Timeframe, strategy: string, minConf = 0.6) =>
    jpost<SignalItem[]>(`/api/signals/watchlist`, { codes, tf, strategy, min_conf: minConf }),
  universe: (params: { market?: string; minPrice?: number; maxPrice?: number; q?: string; limit?: number } = {}) => {
    const u = new URLSearchParams();
    if (params.market) u.set('market', params.market);
    if (params.minPrice != null) u.set('min_price', String(params.minPrice));
    if (params.maxPrice != null) u.set('max_price', String(params.maxPrice));
    if (params.q) u.set('q', params.q);
    if (params.limit != null) u.set('limit', String(params.limit));
    return jget<{ total: number; items: UniverseItem[] }>(`/api/universe?${u.toString()}`);
  },
  // 관심종목 전체 × 전략 프리셋(트레이더) 매트릭스 검증 + 베스트 전략 판정
  strategyMatrix: (body: Record<string, unknown>) => jpost<any>(`/api/backtest/strategy-matrix`, body),
  // 관심종목 패턴별 통계(참고용)
  batchBacktest: (body: Record<string, unknown>) => jpost<any>(`/api/backtest/batch`, body),
  presets: () => jget<Record<string, any>>(`/api/trading/presets`),
  tradingStart: (body: Record<string, unknown>, force = false) =>
    jpost<any>(`/api/trading/start?force=${force}`, body),
  tradingStop: () => jpost<any>(`/api/trading/stop`, {}),
  tradingStatus: () => jget<any>(`/api/trading/status`),
  updateStrategy: (body: Record<string, unknown>) => jput<any>(`/api/trading/strategy`, body),
  readiness: () => jget<ReadinessReport>(`/api/trading/readiness`),
  journal: (mode = 'paper', limit = 200) => jget<any[]>(`/api/trading/journal?mode=${mode}&limit=${limit}`),
  tradeStats: (mode = 'all') => jget<TradeStats>(`/api/trading/stats?mode=${mode}`),
  closePosition: (code: string) =>
    jpost<{ ok: boolean; fill_price: number; qty: number; pnl: number; pnl_pct: number; status: any }>(
      `/api/trading/positions/${code}/close`, {}
    ),
  clearEvents: () => jpost<{ ok: boolean; removed: number; status: any }>(`/api/trading/events/clear`, {}),
  clearJournal: (mode = 'all') => jpost<{ ok: boolean; removed: number }>(`/api/trading/journal/clear?mode=${mode}`, {}),
};

export interface CriterionResult {
  key: string;
  label: string;
  passed: boolean;
  actual: number;
  required: number;
}
export interface ReadinessReport {
  ready: boolean;
  criteria: CriterionResult[];
  stats: Record<string, number>;
  force_allowed: boolean;
}
