import { browser } from '$app/environment';

const KEY = 'tradingSettings';

/** Persisted auto-trading form settings (everything the user configures on the page). */
export interface TradingSettings {
  mode: string;
  strategy: string;
  tf: string;
  watchlistText: string;
  pollSec: number;
  ignoreHours: boolean;
  orderType: string;
  fixedQty: number;
  sellAll: boolean;
  /** 모의투자 매수(총 진입금액) 한도액. */
  maxBuyAmount: number;
  /** 실전투자 매수(총 진입금액) 한도액 — 0이면 아직 계좌 잔고로 초기화되지 않은 상태. */
  maxBuyAmountLive: number;
  stopLossPct: number;
  takeProfitPct: number;
  lossCooldownBars: number;
  reentryCooldownBars: number;
  reentryGapPct: number;
  fibEnabled: boolean;
  fibMaxLevels: number;
  requireConfirmation: boolean;
  requireHigherTfUptrend: boolean;
  minHoldBars: number;
  hardStopIntrabar: boolean;
  hardStopBufferPct: number;
  requireTradeable: boolean;
  weights: Record<string, number>;
  entryThreshold: number;
}

/** Read saved settings (partial — only keys the user has touched). Empty on SSR. */
export function loadTradingSettings(): Partial<TradingSettings> {
  if (!browser) return {};
  try {
    return JSON.parse(localStorage.getItem(KEY) || '{}') as Partial<TradingSettings>;
  } catch {
    return {};
  }
}

/** Persist the current settings as the defaults for next time. */
export function saveTradingSettings(s: TradingSettings): void {
  if (browser) localStorage.setItem(KEY, JSON.stringify(s));
}
