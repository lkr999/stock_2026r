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
  /** 동시 보유 가능한 최대 종목 수. */
  maxPositions: number;
  /** 모의투자 매수(총 진입금액) 한도액. */
  maxBuyAmount: number;
  /** 실전투자 매수(총 진입금액) 한도액 — 0이면 아직 계좌 잔고로 초기화되지 않은 상태. */
  maxBuyAmountLive: number;
  stopLossPct: number;
  takeProfitPct: number;
  /** 자동 매도(청산) 조건 on/off. */
  useStopLoss: boolean;
  useTakeProfit: boolean;
  useTrailingStop: boolean;
  /** 트레일링 스탑 ATR 배수 (고점 대비 ATR×이값 하락 시 청산). */
  trailingStopAtr: number;
  lossCooldownBars: number;
  reentryCooldownBars: number;
  reentryGapPct: number;
  /** 손절 후 재매수 가격가드 자동 해제 봉수 (0 = 무기한). */
  reentryGuardExpireBars: number;
  fibEnabled: boolean;
  fibMaxLevels: number;
  requireConfirmation: boolean;
  /** 확인봉 유효기간 — 패턴 후 이 봉수 안에 확인되면 진입. */
  confirmWindowBars: number;
  requireHigherTfUptrend: boolean;
  /** 상위TF 필터 완화: 봉당 이 %까지의 하락 기울기는 허용 (0 = 엄격). */
  higherTfTolerancePct: number;
  minHoldBars: number;
  hardStopIntrabar: boolean;
  hardStopBufferPct: number;
  /** 장 마감 전 강제 청산 (15:05 진입중단 / 15:10 전량청산). */
  eodFlatten: boolean;
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
