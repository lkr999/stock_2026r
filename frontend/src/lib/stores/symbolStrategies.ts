import { writable } from 'svelte/store';
import { browser } from '$app/environment';

// 종목별 전략 배정 (code → 전략 이름). 대시보드의 백테스트 기반 선정에서 채워지며,
// 자동매매 시작 시 종목마다 다른 전략으로 적용된다. 배정이 없는 종목은 전역 전략을 따른다.
const KEY = 'symbolStrategies';

function load(): Record<string, string> {
  if (!browser) return {};
  try {
    return JSON.parse(localStorage.getItem(KEY) || '{}');
  } catch {
    return {};
  }
}

function createSymbolStrategies() {
  const { subscribe, set, update } = writable<Record<string, string>>(load());

  function persist(v: Record<string, string>) {
    if (browser) localStorage.setItem(KEY, JSON.stringify(v));
  }

  return {
    subscribe,
    /** Replace the whole map (used by backtest-based selection). */
    replace: (map: Record<string, string>) => {
      persist(map);
      set(map);
    },
    /** Merge in assignments without dropping existing ones. */
    setMany: (map: Record<string, string>) =>
      update((v) => {
        const next = { ...v, ...map };
        persist(next);
        return next;
      }),
    setOne: (code: string, strategy: string) =>
      update((v) => {
        const next = { ...v, [code]: strategy };
        persist(next);
        return next;
      }),
    remove: (code: string) =>
      update((v) => {
        const { [code]: _drop, ...rest } = v;
        persist(rest);
        return rest;
      }),
    /** Keep only the given codes (prune assignments for removed watchlist items). */
    prune: (codes: string[]) =>
      update((v) => {
        const next = Object.fromEntries(Object.entries(v).filter(([c]) => codes.includes(c)));
        persist(next);
        return next;
      }),
    clear: () => {
      persist({});
      set({});
    }
  };
}

export const symbolStrategies = createSymbolStrategies();
