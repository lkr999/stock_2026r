import { writable } from 'svelte/store';
import type { Timeframe } from '$lib/api';

export const TIMEFRAME_LABELS: Record<Timeframe, string> = {
  '1m': '1분',
  '3m': '3분',
  '5m': '5분',
  '10m': '10분',
  '15m': '15분',
  '30m': '30분',
  '60m': '60분',
  '1d': '일봉'
};

export const selectedTimeframe = writable<Timeframe>('5m');
export const selectedStrategy = writable<string>('balanced');
