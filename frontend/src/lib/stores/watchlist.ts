import { writable } from 'svelte/store';
import { browser } from '$app/environment';

const KEY = 'watchlist';

function load(): string[] {
  if (!browser) return [];
  try {
    return JSON.parse(localStorage.getItem(KEY) || '[]');
  } catch {
    return [];
  }
}

function createWatchlist() {
  const { subscribe, set, update } = writable<string[]>(load());

  function persist(v: string[]) {
    if (browser) localStorage.setItem(KEY, JSON.stringify(v));
  }

  return {
    subscribe,
    toggle: (code: string) =>
      update((v) => {
        const next = v.includes(code) ? v.filter((c) => c !== code) : [...v, code];
        persist(next);
        return next;
      }),
    add: (code: string) =>
      update((v) => {
        if (v.includes(code)) return v;
        const next = [...v, code];
        persist(next);
        return next;
      }),
    remove: (code: string) =>
      update((v) => {
        const next = v.filter((c) => c !== code);
        persist(next);
        return next;
      }),
    clear: () => {
      persist([]);
      set([]);
    }
  };
}

export const watchlist = createWatchlist();
