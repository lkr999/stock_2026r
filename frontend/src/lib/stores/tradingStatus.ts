import { writable } from 'svelte/store';
import { browser } from '$app/environment';
import { api } from '$lib/api';

// 자동매매 엔진의 실행 여부를 모든 페이지(네비게이션 바·대시보드)에서 확인할 수 있도록
// 주기적으로 폴링해 공유하는 경량 상태. /trading 페이지 자체는 더 자세한 상태를
// 별도로 폴링하므로, 이 스토어는 "진행중인지" 표시 용도로만 최소 필드를 담는다.
export interface TradingStatusLite {
  running: boolean;
  mode?: string;
  watchlist?: string[];
  symbol_strategies?: Record<string, { strategy: string; tf: string }>;
  positions?: { code: string }[];
  cycles?: number;
}

function createTradingStatus() {
  const { subscribe, set } = writable<TradingStatusLite>({ running: false });
  let timer: ReturnType<typeof setInterval> | null = null;

  async function refresh() {
    try {
      set(await api.tradingStatus());
    } catch {
      // 서버 연결 실패 시 마지막으로 알려진 상태를 유지한다.
    }
  }

  function start(intervalMs = 10000) {
    if (!browser || timer) return;
    refresh();
    timer = setInterval(refresh, intervalMs);
  }

  return { subscribe, refresh, start };
}

export const tradingStatus = createTradingStatus();
