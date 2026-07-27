import { browser } from '$app/environment';
import type { UniverseItem } from '$lib/api';

// 대시보드(백테스트 기반 관심종목 선정)의 설정값 + 결과값을 localStorage 에 보존한다.
// 페이지를 떠났다가 돌아와도 스캔 후보·백테스트 격자·선정 옵션이 초기화되지 않는다.
const KEY = 'dashboardState';

/** 대시보드에서 사용자가 설정한 옵션과 스캔/백테스트 결과 전체. */
export interface DashboardState {
  // 스텝 ① 가격 필터
  market: string;
  minPrice: number | undefined;
  maxPrice: number | undefined;
  candidateLimit: number;
  candidates: UniverseItem[];
  scanned: boolean;
  // 스텝 ② 백테스트
  btTf: string;
  btHold: number;
  matrix: any;
  // 스텝 ③ OOS 선정 기준
  oosMinReturn: number;
  oosMinSignals: number;
  oosMinConsistency: number;
  oosRequireTradeable: boolean;
  oosRankBy: string;
  oosMaxPick: number;
  oosSelectMsg: string;
  // eBest 패널
  testCode: string;
  apiPanelOpen: boolean;
}

/** 저장된 대시보드 상태 (건드린 키만 존재). SSR 에서는 빈 객체. */
export function loadDashboardState(): Partial<DashboardState> {
  if (!browser) return {};
  try {
    return JSON.parse(localStorage.getItem(KEY) || '{}') as Partial<DashboardState>;
  } catch {
    return {};
  }
}

/** 현재 대시보드 상태를 다음 방문을 위해 저장. */
export function saveDashboardState(s: DashboardState): void {
  if (browser) localStorage.setItem(KEY, JSON.stringify(s));
}
