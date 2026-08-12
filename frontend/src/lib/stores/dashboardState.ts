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
  /** 검증할 전략 세대 — 'v2' | 'legacy' | 'all'(비교). */
  btGeneration: 'v2' | 'legacy' | 'all';
  btTf: string;
  btHold: number;
  /** 백테스트가 시뮬레이션할 고정 손절 %(0 = ATR 배수). 자동매매 폼과 같은 의미. */
  btStopLossPct: number;
  /** 백테스트가 시뮬레이션할 고정 익절 %(0 = ATR 배수). */
  btTakeProfitPct: number;
  /** 손절을 봉 마감이 아닌 실시간가로 판정할지 — OFF면 실거래의 손절 오버슛을 재현. */
  btHardStopIntrabar: boolean;
  /** 심층 조회 봉수 (0 = 타임프레임 기본 예산). */
  btHistoryBars: number;
  /** 익절 목표폭(%) — 종목별 권장 보유봉수 역산에 사용. */
  btTargetPct: number;
  matrix: any;
  // 스텝 ③ OOS 선정 기준
  oosMinReturn: number;
  oosMinSignals: number;
  oosMinConsistency: number;
  oosRequireTradeable: boolean;
  oosRankBy: string;
  oosMaxPick: number;
  oosSelectMsg: string;
  /** 선정·배정에 사용할 전략 세대 (격자에 두 세대가 있어도 한 세대로만 배정). */
  pickGeneration: 'v2' | 'legacy';
  // 승률 중심 선정 기준
  /** 최소 OOS 승률(%). */
  oosMinWinRate: number;
  /** 최소 승률 여유(%p) = 실제승률 − 손익분기승률. */
  oosMinWinEdge: number;
  /** 최소 손익비 (평균이익 ÷ 평균손실). */
  oosMinPayoff: number;
  /** 최소 IS→OOS 수익 유지율(%) — 과최적화 배제. */
  oosMinRetention: number;
  /** OOS 최악 폴드 낙폭 허용치(%, 0 = 제한없음). */
  oosMaxMdd: number;
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
