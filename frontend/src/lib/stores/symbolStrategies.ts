import { writable } from 'svelte/store';
import { browser } from '$app/environment';

// 종목별 전략 배정 (code → {전략 이름, 백테스트에 실제 사용된 타임프레임}).
// 대시보드의 백테스트 기반 선정에서 채워지며, 자동매매 시작 시 종목마다 다른 전략으로
// 적용된다. 배정이 없는 종목은 전역 전략/전역 TF 를 따른다.
//
// tf 를 함께 저장하는 이유: 백테스트를 특정 TF(예: 5m)로 돌려 고른 전략을,
// 자동매매에서는 프리셋 기본 TF(예: 1m)로 돌려버리면 "백테스트한 것과 다른 조건"으로
// 매매하게 된다. 배정 시점의 TF 를 보존해 백테스트 ↔ 자동매매 조건을 일치시킨다.
const KEY = 'symbolStrategies';

export interface SymbolStrategy {
  /** 전략 프리셋 이름 */
  strategy: string;
  /** 이 종목을 이 전략으로 백테스트했을 때 실제 사용된 타임프레임 (예: '5m', '1m') */
  tf?: string;
}

type AssignMap = Record<string, SymbolStrategy>;

// 구버전(code → "전략이름" 문자열)으로 저장된 값도 읽어 들일 수 있게 정규화한다.
function normalize(raw: unknown): AssignMap {
  if (!raw || typeof raw !== 'object') return {};
  const out: AssignMap = {};
  for (const [code, v] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof v === 'string') out[code] = { strategy: v };
    else if (v && typeof v === 'object' && typeof (v as any).strategy === 'string')
      out[code] = { strategy: (v as any).strategy, tf: (v as any).tf };
  }
  return out;
}

function load(): AssignMap {
  if (!browser) return {};
  try {
    return normalize(JSON.parse(localStorage.getItem(KEY) || '{}'));
  } catch {
    return {};
  }
}

function createSymbolStrategies() {
  const { subscribe, set, update } = writable<AssignMap>(load());

  function persist(v: AssignMap) {
    if (browser) localStorage.setItem(KEY, JSON.stringify(v));
  }

  return {
    subscribe,
    /** Replace the whole map (used by backtest-based selection). */
    replace: (map: AssignMap) => {
      persist(map);
      set(map);
    },
    /** Merge in assignments without dropping existing ones. */
    setMany: (map: AssignMap) =>
      update((v) => {
        const next = { ...v, ...map };
        persist(next);
        return next;
      }),
    setOne: (code: string, entry: SymbolStrategy) =>
      update((v) => {
        const next = { ...v, [code]: entry };
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
