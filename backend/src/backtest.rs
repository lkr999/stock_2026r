//! Cost-aware, look-ahead-safe backtester + walk-forward (spec sections 6-3, 11-1).

use crate::candle::Candle;
use crate::pattern::{
    apply_strategy, compute_atr, detect_setups, detect_v2_setups, trend_slope, PatternDetector,
    PatternResult, SetupSeries,
};
use crate::risk::RiskConfig;
use crate::session::SessionContext;
use crate::strategy::StrategyConfig;
use crate::timeframe::Timeframe;
use serde_json::{json, Value};

/// Korean equity round-trip cost model (fee + tax + slippage).
#[derive(Clone, Copy)]
pub struct CostModel {
    pub fee_rate: f64,
    pub tax_rate: f64,
    pub slippage_rate: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self { fee_rate: 0.00015, tax_rate: 0.0015, slippage_rate: 0.0008 }
    }
}

impl CostModel {
    /// Round-trip cost fraction: 2× fee + sell tax + 2× slippage.
    pub fn round_trip_cost(&self) -> f64 {
        self.fee_rate * 2.0 + self.tax_rate + self.slippage_rate * 2.0
    }
}

/// Performance stats over a list of per-trade net returns (%).
fn summarize(returns: &[f64], extra: Value) -> Value {
    let mut base = json!({
        "signals": 0, "win_rate": 0.0, "avg_return": 0.0,
        "max_drawdown": 0.0, "sharpe_ratio": 0.0, "profit_factor": 0.0,
    });
    if let Value::Object(m) = &extra {
        for (k, v) in m {
            base[k] = v.clone();
        }
    }
    if returns.is_empty() {
        return base;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let wins: f64 = returns.iter().filter(|&&r| r > 0.0).sum();
    let losses: f64 = returns.iter().filter(|&&r| r <= 0.0).sum();
    let win_count = returns.iter().filter(|&&r| r > 0.0).count();
    // Max drawdown of the cumulative equity curve.
    let (mut equity, mut peak, mut mdd): (f64, f64, f64) = (0.0, f64::MIN, 0.0);
    for &r in returns {
        equity += r;
        peak = peak.max(equity);
        mdd = mdd.min(equity - peak);
    }
    let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
    let std = var.sqrt();
    let pf = if losses != 0.0 { wins / losses.abs() } else { f64::INFINITY };
    base["signals"] = json!(returns.len());
    base["win_rate"] = json!(win_count as f64 / n);
    base["avg_return"] = json!(mean);
    base["max_drawdown"] = json!(mdd);
    base["sharpe_ratio"] = json!(mean / (std + 1e-9));
    base["profit_factor"] = json!(pf);
    // 폴드 간 합산이 가능한 원시 집계값 — walk-forward 는 폴드별 요약의 평균이
    // 아니라 *전 폴드의 거래를 한 풀로 모아* 승률·손익비를 내야 표본이 적은
    // 폴드가 과대 반영되지 않는다.
    base["win_count"] = json!(win_count);
    base["sum_win"] = json!(wins);
    base["sum_loss"] = json!(losses.abs());
    base
}

/// 승률의 Wilson 하한 (단측 95% 신뢰). 표본이 적을수록 크게 깎이므로
/// "3전 3승(100%)" 이 "100전 70승(70%)" 보다 아래로 내려간다 — 소표본 과대평가를
/// 막는 표준적인 방법이며, 관심종목 랭킹의 기본 기준으로 쓴다.
pub fn wilson_lower_bound(wins: u64, n: u64) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let z = 1.645_f64; // 95% one-sided
    let (w, nf) = (wins as f64, n as f64);
    let p = w / nf;
    let z2 = z * z;
    let denom = 1.0 + z2 / nf;
    let centre = p + z2 / (2.0 * nf);
    let margin = z * ((p * (1.0 - p) + z2 / (4.0 * nf)) / nf).sqrt();
    ((centre - margin) / denom).clamp(0.0, 1.0)
}

/// 봉당 종가수익률 표준편차(%) — "이 종목이 한 봉에 평균 몇 % 움직이는가".
/// 목표폭 도달에 필요한 봉수를 (목표% / σ)² 로 추정하는 데 쓴다.
pub fn bar_sigma_pct(candles: &[Candle]) -> f64 {
    let rets: Vec<f64> = candles
        .windows(2)
        .filter(|w| w[0].close > 0.0)
        .map(|w| (w[1].close - w[0].close) / w[0].close * 100.0)
        .collect();
    if rets.len() < 2 {
        return 0.0;
    }
    let m = rets.iter().sum::<f64>() / rets.len() as f64;
    (rets.iter().map(|r| (r - m).powi(2)).sum::<f64>() / rets.len() as f64).sqrt()
}

/// 목표폭 `target_pct` 에 닿기까지 기대되는 봉수 = (목표 / σ)², 안전계수 1.5배.
/// σ 를 알 수 없으면 0 (권장값 없음).
pub fn recommended_hold_bars(target_pct: f64, sigma_pct: f64) -> usize {
    if sigma_pct <= 1e-9 || target_pct <= 0.0 {
        return 0;
    }
    ((target_pct / sigma_pct).powi(2) * 1.5).ceil() as usize
}

// ---------------------------------------------------------------------------
// 상위 타임프레임 컨텍스트 (백테스트를 실거래와 일치시키기 위한 것)
// ---------------------------------------------------------------------------

/// 베이스 봉 인덱스로 바로 조회할 수 있게 미리 계산해 둔 상위TF 상태.
///
/// 실거래 엔진(`try_enter`)은 진입 직전에 상위 타임프레임을 조회해 ① MTF 컨플루언스
/// 점수를 종합점수에 반영하고 ② 상위TF가 하락추세면 진입을 차단한다. 백테스트에
/// 이 둘이 없으면 **종합점수와 진입 횟수가 실거래와 달라져** OOS 통계가 다른 전략을
/// 재는 셈이 된다 (특히 MTF 가중치 0.30 인 `balanced`/`conservative` 계열).
///
/// 상위TF 캔들을 따로 조회하지 않고 **베이스 봉을 리샘플링**해 만든다 —
/// 747종목 배치에서 호출 수가 2~3배로 늘어나는 것을 피하면서도, 봉 `i` 시점에
/// *이미 마감된* 상위 봉만 사용하므로 미래참조가 없다.
pub struct MtfContext {
    /// 봉 `i` 시점의 MTF 컨플루언스 점수 (상위TF 중 신호가 뜬 비율, 상위TF 없으면 0.5).
    pub score: Vec<f64>,
    /// 봉 `i` 시점에 최근접 상위TF가 하락추세가 아닌가 (진입 허용 여부).
    pub uptrend: Vec<bool>,
}

/// 한 봉의 "장중 분(分)" — 리샘플 버킷 계산용. 파싱 실패 시 0.
fn minute_of_day(c: &Candle) -> i64 {
    let t: String = c.time.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if t.len() < 4 {
        return 0;
    }
    let hh: i64 = t[..2].parse().unwrap_or(0);
    let mm: i64 = t[2..4].parse().unwrap_or(0);
    hh * 60 + mm
}

/// 베이스 봉을 상위 타임프레임으로 합친다.
/// 반환: (상위TF 봉들, 베이스 봉 `i` 시점에 *마감 완료된* 상위 봉 개수).
///
/// 마감 개수를 함께 돌려주는 것이 핵심이다 — 형성 중인 상위 봉을 쓰면
/// 그 봉의 미래(나머지 베이스 봉들)를 미리 보는 셈이 된다.
fn resample(candles: &[Candle], target: Timeframe) -> (Vec<Candle>, Vec<usize>) {
    let step = match target {
        Timeframe::D1 => 0, // 날짜만으로 묶는다
        _ => target.config().ncnt as i64,
    };
    let bucket = |c: &Candle| -> (String, i64) {
        if step <= 0 {
            (c.date.clone(), 0)
        } else {
            (c.date.clone(), minute_of_day(c) / step)
        }
    };

    let mut bars: Vec<Candle> = Vec::new();
    let mut completed = vec![0usize; candles.len()];
    let mut cur: Option<(String, i64)> = None;
    for (i, c) in candles.iter().enumerate() {
        let k = bucket(c);
        if cur.as_ref() != Some(&k) {
            bars.push(c.clone()); // 새 상위 봉 시작 (open/high/low/close/volume 승계)
            cur = Some(k);
        } else {
            let last = bars.last_mut().expect("pushed above");
            last.high = last.high.max(c.high);
            last.low = last.low.min(c.low);
            last.close = c.close;
            last.volume += c.volume;
        }
        // 마지막 봉은 아직 형성 중 → 마감된 것은 그 앞까지.
        completed[i] = bars.len() - 1;
    }
    (bars, completed)
}

impl MtfContext {
    /// 상위TF가 없는 경우(일봉 등) — 점수 중립 0.5, 추세 게이트 통과.
    pub fn neutral(n: usize) -> Self {
        Self { score: vec![0.5; n], uptrend: vec![true; n] }
    }

    /// `tolerance` = `RiskConfig::higher_tf_slope_tolerance` (엔진과 같은 의미).
    pub fn build(candles: &[Candle], tf: Timeframe, tolerance: f64) -> Self {
        let n = candles.len();
        let uppers = tf.mtf_group();
        if uppers.is_empty() || n == 0 {
            return Self::neutral(n);
        }
        let detector = PatternDetector;
        let default_cfg = StrategyConfig::default();
        let mut score = vec![0.0f64; n];
        let mut uptrend = vec![true; n];

        for (u_i, upper) in uppers.iter().enumerate() {
            let (bars, completed) = resample(candles, *upper);
            // 상위 봉 j 가 마지막 마감봉일 때 신호가 있는가 (엔진 `MtfEngine::score` 와 동일 조건).
            let hit: Vec<bool> = (0..bars.len())
                .map(|j| !detector.scan(&bars[..=j], *upper, 0.5, true, &default_cfg, 1).is_empty())
                .collect();
            // 최근접 상위TF만 추세 게이트에 쓴다 (엔진 `higher_tf_uptrend` 와 동일).
            let trend: Vec<bool> = (0..bars.len())
                .map(|j| {
                    if j + 1 < 6 {
                        return true; // 엔진과 동일: 봉이 6개 미만이면 판단 보류
                    }
                    let lo = (j + 1).saturating_sub(20);
                    let tail = &bars[lo..=j];
                    let mean = tail.iter().map(|c| c.close).sum::<f64>() / tail.len() as f64;
                    if mean <= 0.0 {
                        return true;
                    }
                    trend_slope(tail) / mean >= -tolerance.max(0.0)
                })
                .collect();

            for i in 0..n {
                let done = completed[i]; // 마감된 상위 봉 개수
                if done == 0 {
                    // 아직 마감된 상위 봉이 없다 → 중립 취급 (엔진도 데이터 부족 시 통과).
                    score[i] += 0.5;
                    continue;
                }
                let j = done - 1;
                if hit[j] {
                    score[i] += 1.0;
                }
                if u_i == 0 {
                    uptrend[i] = trend[j];
                }
            }
        }
        let k = uppers.len() as f64;
        for s in &mut score {
            *s /= k;
        }
        Self { score, uptrend }
    }

    /// `[a, b)` 구간을 잘라낸 새 컨텍스트 (walk-forward 폴드용).
    /// 값은 이미 인과적으로 계산돼 있으므로 잘라도 미래참조가 생기지 않는다.
    pub fn slice(&self, a: usize, b: usize) -> Self {
        Self { score: self.score[a..b].to_vec(), uptrend: self.uptrend[a..b].to_vec() }
    }
}

/// Pattern-only backtest: buy at next bar's open, sell after fixed `hold_bars`.
pub fn run_backtest(
    candles: &[Candle],
    pattern_name: &str,
    tf: Timeframe,
    hold_bars: usize,
    cost: &CostModel,
    detector: &PatternDetector,
) -> Value {
    let rt_cost = cost.round_trip_cost() * 100.0;
    let default_cfg = StrategyConfig::default();
    let mut returns: Vec<f64> = vec![];
    if candles.len() <= hold_bars + 1 {
        return summarize(&returns, json!({"pattern": pattern_name}));
    }
    for i in 0..candles.len() - hold_bars - 1 {
        let start = i.saturating_sub(40);
        let window = &candles[start..=i];
        if window.len() < 13 {
            continue;
        }
        let found = detector.scan(window, tf, 0.0, false, &default_cfg, 1);
        if !found.iter().any(|p| p.pattern_name == pattern_name) {
            continue;
        }
        let entry = candles[i + 1].open;
        if entry <= 0.0 {
            continue;
        }
        let exit = candles[i + 1 + hold_bars].close;
        let gross = (exit - entry) / entry * 100.0;
        returns.push(gross - rt_cost);
    }
    summarize(&returns, json!({"pattern": pattern_name}))
}

/// Aggregate `run_backtest` across many patterns (signal-weighted).
pub fn backtest_many(candles: &[Candle], patterns: &[String], tf: Timeframe, hold_bars: usize, cost: &CostModel) -> Value {
    let detector = PatternDetector;
    let by_pattern: Vec<Value> = patterns
        .iter()
        .map(|p| run_backtest(candles, p, tf, hold_bars, cost, &detector))
        .collect();
    let sig = |b: &Value| b.get("signals").and_then(Value::as_u64).unwrap_or(0) as f64;
    let total: f64 = by_pattern.iter().map(sig).sum();
    let (avg, win) = if total == 0.0 {
        (0.0, 0.0)
    } else {
        let avg = by_pattern.iter().map(|b| b["avg_return"].as_f64().unwrap_or(0.0) * sig(b)).sum::<f64>() / total;
        let win = by_pattern.iter().map(|b| b["win_rate"].as_f64().unwrap_or(0.0) * sig(b)).sum::<f64>() / total;
        (avg, win)
    };
    json!({
        "total_signals": total as u64,
        "avg_return": avg,
        "win_rate": win,
        "by_pattern": by_pattern,
    })
}

/// Indicator warmup a fold must be given before its first scored bar.
///
/// The slowest input any detector reads is EMA50 (the v2 trend filter), so a
/// window shorter than this produces no v2 signals *at all* — which previously
/// made every v2 strategy look untradeable in walk-forward, since the folds
/// were shorter than the warmup. Walk-forward slices now carry this much
/// leading history, which is causal (it is strictly older data) and therefore
/// introduces no look-ahead.
pub const WARMUP_BARS: usize = 55;

/// As-traded backtest: same gates + ATR stop/target/trailing as the live engine.
///
/// A v2 strategy is simulated under **its own** risk doctrine, not the caller's
/// form values: `cfg.effective_risk` applies the strategy's overrides first, so
/// the backtest exercises the ATR stop and intrabar fill model the engine will
/// actually use. The one live gate not modelled here is the market filter
/// (§`crate::market`), which needs proxy candles the backtester isn't given —
/// live entries are therefore a strict *subset* of the entries counted here.
///
/// `skip_bars` leading bars are used to warm indicators up but never scored, so
/// a walk-forward fold can be handed preceding history without counting trades
/// that belong to the previous fold.
#[allow(clippy::too_many_arguments)]
pub fn run_strategy_backtest_from(
    candles: &[Candle],
    cfg: &StrategyConfig,
    tf: Timeframe,
    risk: &RiskConfig,
    cost: &CostModel,
    max_hold_bars: usize,
    mtf: &MtfContext,
    skip_bars: usize,
) -> Value {
    let effective = cfg.effective_risk(risk);
    let risk = &effective;
    let detector = PatternDetector;
    let rt_cost = cost.round_trip_cost() * 100.0;
    let cost_pct = rt_cost;
    let n = candles.len();
    // Session-anchored context for VWAP/ORB/EMA/RSI/BB setups, precomputed once.
    let ctx = SessionContext::for_tf(candles, tf);
    let series = SetupSeries::compute(candles);
    let mut returns: Vec<f64> = vec![];
    let mut rr_values: Vec<f64> = vec![];
    let mut i = skip_bars;
    while i + 1 < n {
        // Candidate pool = windowed candlestick patterns + context setups.
        let start = i.saturating_sub(40);
        let window = &candles[start..=i];
        let mut cands: Vec<PatternResult> =
            if window.len() >= 13 { detector.scan(window, tf, 0.0, true, cfg, 1) } else { vec![] };
        let mut setups = detect_setups(candles, i, &ctx, &series, &cfg.enabled_patterns);
        setups.extend(detect_v2_setups(candles, i, &ctx, &series, &cfg.enabled_patterns));
        cands.append(&mut setups);

        // 상위TF 추세 게이트 — 엔진 `try_enter` 와 동일하게, 상위TF가 하락추세면
        // 신호가 있어도 진입하지 않는다. 이걸 빼면 백테스트만 진입을 과다 계상한다.
        if risk.require_higher_tf_uptrend && !mtf.uptrend.get(i).copied().unwrap_or(true) {
            i += 1;
            continue;
        }
        // MTF 컨플루언스 점수를 종합점수에 반영한다 (엔진과 동일: has_mtf = true).
        // 예전에는 false 로 계산해 MTF 가중치가 있는 프리셋은 실거래와 다른
        // 종합점수를 쓰고 있었다 — 진입 시점 자체가 달라진다.
        let mtf_score = mtf.score.get(i).copied().unwrap_or(0.5);
        for r in &mut cands {
            r.mtf_score = mtf_score;
            apply_strategy(r, cfg, true, false, false);
        }

        let best = cands
            .into_iter()
            .filter(|r| {
                cfg.enabled_patterns.contains(&r.pattern_name)
                    && r.composite_score >= cfg.entry_threshold
                    && (!cfg.require_volume_confirm || r.volume_confirmed)
            })
            .max_by(|a, b| a.composite_score.partial_cmp(&b.composite_score).unwrap_or(std::cmp::Ordering::Equal));
        let Some(top) = best else {
            i += 1;
            continue;
        };
        // 확인봉 게이트 — 엔진(try_enter)과 동일: 패턴 후 `confirm_window_bars`
        // 봉 안에 확인봉(패턴 고점 돌파 or 방향 일치 종가)이 나와야 그 *다음*
        // 봉 시가에 진입한다. 미확인 시 진입 없음. 이 게이트를 빼면 확인봉 ON
        // 상태의 실거래와 OOS 통계가 어긋난다.
        let mut entry_idx = i + 1;
        if risk.require_confirmation {
            let p_high = top.candles_used.iter().map(|c| c.high).fold(f64::MIN, f64::max);
            let p_close = top.candles_used.last().map(|c| c.close).unwrap_or(0.0);
            let win = risk.confirm_window_bars.max(1) as usize;
            let hi = (i + win).min(n.saturating_sub(2)); // 확인봉 다음 봉에 진입해야 하므로 n-2까지
            let confirmed = (i + 1..=hi).find(|&j| {
                let b = &candles[j];
                b.close > p_high || (b.is_bull() && b.close > p_close)
            });
            match confirmed {
                Some(j) => entry_idx = j + 1,
                None => {
                    i += 1;
                    continue;
                }
            }
        }
        let entry = candles[entry_idx].open;
        let atr = compute_atr(&candles[..entry_idx], 14);
        if entry <= 0.0 || atr <= 0.0 {
            i += 1;
            continue;
        }
        // Actual stop/target distances — manual fixed-% takes priority over ATR
        // multiples, exactly as the live engine places them (risk.stop_and_target).
        let (stop_dist, target_dist) = risk.stop_target_dists(entry, atr);
        if stop_dist <= 0.0 {
            i += 1;
            continue;
        }
        // Reward/risk + edge-over-cost gates (same as engine `passes_entry_gates`).
        let rr = target_dist / stop_dist;
        if cfg.min_reward_risk > 0.0 && rr < cfg.min_reward_risk {
            i += 1;
            continue;
        }
        let target_move_pct = target_dist / entry * 100.0;
        if cfg.min_edge_over_cost > 0.0 && target_move_pct < cost_pct * cfg.min_edge_over_cost {
            i += 1;
            continue;
        }
        let (stop, target) = (entry - stop_dist, entry + target_dist);
        rr_values.push(rr);
        let mut peak = entry; // best excursion: the highest close since entry
        // EOD 강제청산 — 엔진이 15:10 에 전량 청산하므로 백테스트도 세션을 넘기지
        // 않는다. 이게 없으면 백테스트에만 오버나이트 보유가 생겨(실측 손실의 91%가
        // 거기서 나왔다) 결과가 낙관 쪽으로 치우친다.
        let session_end = if risk.eod_flatten {
            let d = &candles[entry_idx].date;
            candles[entry_idx..].iter().position(|c| &c.date != d).map(|k| entry_idx + k - 1)
        } else {
            None
        };
        let hold_cap = session_end.map_or(usize::MAX, |e| e.saturating_sub(entry_idx) + 1).min(max_hold_bars);
        if hold_cap == 0 {
            i += 1;
            continue;
        }
        let last_idx = (entry_idx + hold_cap).min(n - 1);
        let (mut exit_price, mut exit_idx) = (candles[last_idx].close, last_idx);
        for j in entry_idx..(entry_idx + hold_cap).min(n) {
            let c = candles[j].close;
            // 트레일링 폭은 엔진(check_exit)과 동일하게 *현재 봉 기준* ATR 로
            // 매 봉 재계산한다 (손절/익절선은 진입 시점 기준으로 고정).
            let atr_j = {
                let a = compute_atr(&candles[..=j], 14);
                if a > 0.0 { a } else { atr }
            };
            peak = peak.max(c);
            let trail = peak - atr_j * risk.trailing_stop_atr;
            // 체결가 모델 — 엔진과 동일해야 한다.
            //   · hard_stop_intrabar ON : 봉 안에서 스탑선에 걸리면 그 가격 근처 체결
            //   · OFF(기본)            : 닫힌 봉의 *종가*로만 판정·체결 → 스탑을 넘겨
            //                            더 내려간 만큼 그대로 손실 (오버슛)
            // 예전에는 항상 stop/target 가격에 정확히 체결된다고 가정해 손실을
            // 체계적으로 과소평가했다 (실측 손절 평균 -3.49% vs 설정 -3.0%).
            if c <= stop {
                exit_price = if risk.hard_stop_intrabar { stop } else { c };
                exit_idx = j;
                break;
            }
            if c >= target {
                exit_price = if risk.hard_stop_intrabar { target } else { c };
                exit_idx = j;
                break;
            }
            if c <= trail && c > entry {
                exit_price = c;
                exit_idx = j;
                break;
            }
        }
        let gross = (exit_price - entry) / entry * 100.0;
        returns.push(gross - rt_cost);
        i = exit_idx + 1; // resume scanning after the exit (no overlapping positions)
    }
    // Mean per-entry reward/risk (entries can differ when manual % mixes with ATR).
    let rr_mean = if rr_values.is_empty() { 0.0 } else { rr_values.iter().sum::<f64>() / rr_values.len() as f64 };
    summarize(&returns, json!({"strategy": cfg.name, "round_trip_cost_pct": rt_cost, "reward_risk": rr_mean}))
}

/// [`run_strategy_backtest_from`] over the whole series (no warmup skip).
#[allow(clippy::too_many_arguments)]
pub fn run_strategy_backtest(
    candles: &[Candle],
    cfg: &StrategyConfig,
    tf: Timeframe,
    risk: &RiskConfig,
    cost: &CostModel,
    max_hold_bars: usize,
    mtf: &MtfContext,
) -> Value {
    run_strategy_backtest_from(candles, cfg, tf, risk, cost, max_hold_bars, mtf, 0)
}

/// Number of walk-forward folds used by the OOS evaluation.
pub const OOS_FOLDS: usize = 4;

/// `tradeable` 판정에 필요한 최소 OOS 신호 수 — 표본이 이보다 적으면 승률·
/// 기대값을 신뢰할 수 없다고 보고 통과시키지 않는다.
pub const MIN_OOS_SIGNALS: u64 = 10;

/// 폴드 하나가 이보다 짧으면 walk-forward 분할이 의미를 잃으므로,
/// 절반/절반 단일 분할(OOS 표본 1개)로 폴백한다.
const MIN_FOLD_BARS: usize = 15;

/// 주어진 캔들 수에서 walk-forward 가 실제로 만들어내는 (폴드 수, 폴드당 검정봉수).
/// `walk_forward_split` 과 **같은 규칙**을 쓴다 — 화면에 표시하는 예상치가
/// 실제 계산과 어긋나지 않도록 한 곳에서만 정의한다.
pub fn oos_layout(n_candles: usize) -> (usize, usize) {
    let fold = n_candles / (OOS_FOLDS + 1);
    if fold < MIN_FOLD_BARS {
        (1, n_candles - n_candles / 2) // 절반/절반 폴백
    } else {
        (OOS_FOLDS, fold)
    }
}

/// OOS 표본을 만들 수 있는 검정 구간의 총 봉수. 프런트엔드가 "이 타임프레임·
/// 보유봉수로 OOS 신호 10건이 가능한가"를 실행 전에 계산할 수 있도록 노출한다.
pub fn oos_test_bars(n_candles: usize) -> usize {
    let (folds, fold_bars) = oos_layout(n_candles);
    folds * fold_bars
}

/// Walk-forward 검정 구간들의 `[start, end)` 인덱스 (spec section 11-1).
/// 캔들뿐 아니라 상위TF 컨텍스트도 같은 구간으로 잘라야 하므로 인덱스로 돌려준다.
fn walk_forward_split(n: usize, n_folds: usize) -> Vec<(usize, usize)> {
    let fold_size = n / (n_folds + 1);
    if fold_size < MIN_FOLD_BARS {
        return vec![(n / 2, n)];
    }
    (0..n_folds)
        .map(|k| {
            let train_end = fold_size * (k + 1);
            (train_end, (train_end + fold_size).min(n))
        })
        .collect()
}

/// Walk-forward OOS evaluation under live-equivalent conditions (원인 5).
///
/// `max_hold_bars` 는 in-sample 백테스트와 **반드시 같은 값**이어야 한다. 예전에는
/// 여기서 25로 하드코딩돼 있어서, 사용자가 화면에서 보유봉수를 바꿔도 `tradeable`
/// 판정(= OOS 결과)은 전혀 달라지지 않았다.
///
/// 표본 수 관계: 각 검정 폴드는 `n/(OOS_FOLDS+1)` 봉이고, 한 번 진입하면 최대
/// `max_hold_bars` 봉을 소비한 뒤 다음 진입을 찾는다(포지션 중첩 없음). 따라서
/// 폴드당 신호 상한은 `폴드봉수 / max_hold_bars`, 전체 상한은
/// `oos_test_bars(n) / max_hold_bars` 다. 보유봉수가 폴드 길이에 근접하면 폴드당
/// 1건까지 떨어져 `tradeable` 의 최소 표본 10건을 구조적으로 넘길 수 없다.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_strategy_live(
    candles: &[Candle],
    cfg: &StrategyConfig,
    tf: Timeframe,
    risk: &RiskConfig,
    cost: &CostModel,
    max_hold_bars: usize,
    mtf: &MtfContext,
) -> Value {
    let splits = walk_forward_split(candles.len(), OOS_FOLDS);
    let fold_bars = splits.iter().map(|(a, b)| b - a).min().unwrap_or(0);
    let test_bars: usize = splits.iter().map(|(a, b)| b - a).sum();
    // 이 설정으로 이론상 나올 수 있는 최대 OOS 신호 수 — 실제 신호가 적을 때
    // "전략이 나빠서"인지 "구간이 짧아서"인지 화면에서 구분할 수 있게 함께 반환한다.
    let signal_capacity = test_bars / max_hold_bars.max(1);
    let capacity = json!({
        "oos_folds": splits.len(),
        "oos_fold_bars": fold_bars,
        "oos_test_bars": test_bars,
        "oos_signal_capacity": signal_capacity,
        "oos_capacity_ok": signal_capacity >= MIN_OOS_SIGNALS as usize,
    });
    let merge = |mut base: Value| -> Value {
        if let (Some(b), Some(c)) = (base.as_object_mut(), capacity.as_object()) {
            for (k, v) in c {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    };

    let oos: Vec<Value> = splits
        .into_iter()
        .map(|(a, b)| {
            // 폴드 앞에 워밍업 구간을 덧붙인다 — EMA50 같은 느린 지표는 폴드
            // 길이(60봉 안팎)만으로는 예열이 끝나지 않아, 이게 없으면 v2 셋업이
            // 폴드 안에서 한 번도 발동하지 못한다. 덧붙인 구간은 *과거* 데이터라
            // 미래참조가 아니며, `skip` 으로 채점에서 제외해 이전 폴드의 거래가
            // 이번 폴드에 섞이지 않게 한다.
            let w = a.saturating_sub(WARMUP_BARS);
            let skip = a - w;
            // 상위TF 컨텍스트도 같은 구간으로 자른다 — 값은 이미 인과적으로
            // 계산돼 있으므로(봉 i 는 i 이전 데이터만 사용) 미래참조가 없다.
            run_strategy_backtest_from(
                &candles[w..b], cfg, tf, risk, cost, max_hold_bars, &mtf.slice(w, b), skip,
            )
        })
        .filter(|r| r["signals"].as_u64().unwrap_or(0) > 0)
        .collect();
    if oos.is_empty() {
        return merge(json!({"oos_avg_return": 0.0, "oos_consistency": 0.0, "oos_samples": 0,
                            "oos_total_signals": 0, "tradeable": false}));
    }
    let avg = oos.iter().map(|r| r["avg_return"].as_f64().unwrap_or(0.0)).sum::<f64>() / oos.len() as f64;
    let consistency = oos.iter().filter(|r| r["avg_return"].as_f64().unwrap_or(0.0) > 0.0).count() as f64 / oos.len() as f64;
    let f = |r: &Value, k: &str| r[k].as_f64().unwrap_or(0.0);
    let total_sig: u64 = oos.iter().map(|r| r["signals"].as_u64().unwrap_or(0)).sum();
    let win_cnt: u64 = oos.iter().map(|r| r["win_count"].as_u64().unwrap_or(0)).sum();
    let sum_win: f64 = oos.iter().map(|r| f(r, "sum_win")).sum();
    let sum_loss: f64 = oos.iter().map(|r| f(r, "sum_loss")).sum();
    let loss_cnt = total_sig.saturating_sub(win_cnt);
    let win_rate = if total_sig > 0 { win_cnt as f64 / total_sig as f64 } else { 0.0 };
    let avg_win = if win_cnt > 0 { sum_win / win_cnt as f64 } else { 0.0 };
    let avg_loss = if loss_cnt > 0 { sum_loss / loss_cnt as f64 } else { 0.0 };
    // 손익비 = 평균이익 ÷ 평균손실. 이 값과 승률이 함께 있어야 기대값을 읽을 수 있다.
    let payoff = if avg_loss > 1e-9 { avg_win / avg_loss } else { 0.0 };
    // 손익분기 승률 = 1/(1+손익비). 실제 승률이 이보다 높아야 기대값이 양(+).
    let breakeven = if payoff > 1e-9 { 1.0 / (1.0 + payoff) } else { 1.0 };
    let profit_factor = if sum_loss > 1e-9 { sum_win / sum_loss } else { 0.0 };
    // 가장 나쁜 폴드의 낙폭 — 폴드 평균으로 감추면 최악 구간이 보이지 않는다.
    let worst_mdd = oos.iter().map(|r| f(r, "max_drawdown")).fold(0.0_f64, f64::min);
    merge(json!({
        "oos_avg_return": avg,
        "oos_consistency": consistency,
        "oos_samples": oos.len(),
        "oos_total_signals": total_sig,
        "oos_win_rate": win_rate,
        // 소표본 과대평가 방지용 승률 하한 — 관심종목 랭킹의 기본 기준.
        "oos_win_rate_lb": wilson_lower_bound(win_cnt, total_sig),
        "oos_avg_win": avg_win,
        "oos_avg_loss": avg_loss,
        "oos_payoff": payoff,
        "oos_breakeven_win_rate": breakeven,
        // 승률이 손익분기 승률을 얼마나 웃도는가 (양수여야 구조적으로 남는다).
        "oos_win_edge": win_rate - breakeven,
        "oos_profit_factor": profit_factor,
        "oos_worst_mdd": worst_mdd,
        // Recommend live only when OOS net expectancy is positive and consistent.
        "tradeable": avg > 0.0 && consistency >= 0.6 && total_sig >= MIN_OOS_SIGNALS,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(n: usize) -> Vec<Candle> {
        (0..n)
            .map(|i| Candle {
                ts: format!("20260804 {:04}", 900 + i),
                date: "20260804".into(),
                time: format!("{:04}", 900 + i),
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.0,
                volume: 1000.0,
            })
            .collect()
    }

    /// 09:00 부터 1분 간격의 봉 `n` 개 (분→시각 환산이 올바른 시계열).
    fn minute_bars(n: usize) -> Vec<Candle> {
        (0..n)
            .map(|i| {
                let m = 9 * 60 + i;
                let (hh, mm) = (m / 60, m % 60);
                Candle {
                    ts: format!("20260804 {hh:02}{mm:02}"),
                    date: "20260804".into(),
                    time: format!("{hh:02}{mm:02}"),
                    open: 100.0 + i as f64,
                    high: 101.0 + i as f64,
                    low: 99.0 + i as f64,
                    close: 100.5 + i as f64,
                    volume: 1000.0,
                }
            })
            .collect()
    }

    #[test]
    fn resample_aggregates_ohlcv_into_higher_timeframe() {
        let c = minute_bars(12); // 09:00~09:11
        let (bars, completed) = resample(&c, Timeframe::M5);
        // 09:00-09:04 / 09:05-09:09 / 09:10-09:11(형성중) → 3개 버킷
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].open, c[0].open, "시가 = 첫 봉 시가");
        assert_eq!(bars[0].close, c[4].close, "종가 = 마지막 봉 종가");
        assert_eq!(bars[0].high, c[4].high, "고가 = 구간 최고");
        assert_eq!(bars[0].low, c[0].low, "저가 = 구간 최저");
        assert_eq!(bars[0].volume, 5000.0, "거래량 = 합");
        // 마감 개수: 첫 버킷이 진행 중인 동안은 0, 두 번째 버킷 진입 후 1.
        assert_eq!(completed[0], 0);
        assert_eq!(completed[4], 0, "09:04 시점에는 09:00봉이 아직 형성 중");
        assert_eq!(completed[5], 1, "09:05 진입 → 09:00봉 마감");
        assert_eq!(completed[10], 2);
    }

    #[test]
    fn mtf_context_never_uses_future_bars() {
        // 앞부분이 같은 두 시계열이라면, 그 앞부분의 MTF 값도 같아야 한다.
        // (뒤쪽 봉을 보고 계산했다면 값이 달라진다 = 미래참조)
        let short = minute_bars(80);
        let mut long = minute_bars(160);
        // 뒤쪽만 완전히 다른 모양으로 바꾼다.
        for c in long.iter_mut().skip(80) {
            c.close *= 3.0;
            c.high *= 3.0;
        }
        let a = MtfContext::build(&short, Timeframe::M1, 0.0);
        let b = MtfContext::build(&long, Timeframe::M1, 0.0);
        for i in 0..short.len() {
            assert!((a.score[i] - b.score[i]).abs() < 1e-12, "score 미래참조 (i={i})");
            assert_eq!(a.uptrend[i], b.uptrend[i], "uptrend 미래참조 (i={i})");
        }
    }

    #[test]
    fn mtf_context_is_neutral_without_higher_timeframes() {
        // 일봉은 상위TF가 없다 → 점수 중립, 게이트 통과.
        let ctx = MtfContext::build(&bars(30), Timeframe::D1, 0.0);
        assert!(ctx.score.iter().all(|&s| (s - 0.5).abs() < 1e-12));
        assert!(ctx.uptrend.iter().all(|&u| u));
    }

    #[test]
    fn walk_forward_yields_four_equal_test_folds() {
        let splits = walk_forward_split(200, OOS_FOLDS); // 10m 타임프레임의 조회 봉수
        assert_eq!(splits.len(), 4);
        for (a, b) in &splits {
            assert_eq!(b - a, 40, "폴드당 검정 봉수 = n/(folds+1)");
        }
        // 폴드끼리 겹치지 않고 순차적이어야 한다 (구간 중복 = 표본 부풀리기).
        for w in splits.windows(2) {
            assert!(w[0].1 <= w[1].0, "검정 구간이 겹치면 안 된다");
        }
        assert_eq!(oos_test_bars(200), 160, "검정 총봉수 = n × folds/(folds+1)");
    }

    #[test]
    fn short_series_falls_back_to_a_single_split() {
        // 일봉 60개 → fold_size 12 < 15 → 절반/절반 단일 분할 (OOS 표본 1개뿐)
        let splits = walk_forward_split(60, OOS_FOLDS);
        assert_eq!(splits.len(), 1);
        assert_eq!(splits[0].1 - splits[0].0, 30);
        // 화면에 표시하는 예상치도 같은 폴백 규칙을 따라야 한다.
        assert_eq!(oos_layout(60), (1, 30));
        assert_eq!(oos_test_bars(60), 30);
    }

    #[test]
    fn layout_matches_actual_split_for_every_timeframe_budget() {
        // 각 타임프레임의 조회 봉수에 대해 예상 레이아웃 == 실제 분할 결과.
        for n in [500, 300, 200, 100, 60] {
            let splits = walk_forward_split(n, OOS_FOLDS);
            let (folds, fold_bars) = oos_layout(n);
            assert_eq!(splits.len(), folds, "n={n} 폴드 수 불일치");
            assert_eq!(splits.iter().map(|(a, b)| b - a).min().unwrap(), fold_bars, "n={n} 폴드 길이 불일치");
        }
    }

    #[test]
    fn wilson_bound_penalises_small_samples() {
        // 같은 승률이면 표본이 클수록 하한이 높다 (= 소표본에 벌점).
        assert!(wilson_lower_bound(80, 100) > wilson_lower_bound(8, 10));
        assert!(wilson_lower_bound(28, 40) > wilson_lower_bound(7, 10));
        // 극소표본의 100% 는 대표본 70% 를 이기지 못한다 — "3전 3승" 착시 차단.
        assert!(wilson_lower_bound(3, 3) < wilson_lower_bound(70, 100));
        // 하한은 항상 점추정보다 보수적이고, 표본이 커지면 수렴한다.
        assert!(wilson_lower_bound(6000, 10000) < 0.6);
        assert!((wilson_lower_bound(6000, 10000) - 0.6).abs() < 0.02);
        assert_eq!(wilson_lower_bound(0, 0), 0.0);
    }

    #[test]
    fn sigma_and_recommended_hold_are_consistent() {
        // σ 가 목표폭의 절반이면 (2)²×1.5 = 6봉.
        assert_eq!(recommended_hold_bars(2.0, 1.0), 6);
        // 목표를 2배로 넓히면 필요한 봉수는 4배 (제곱 관계).
        assert_eq!(recommended_hold_bars(4.0, 1.0), 24);
        // σ 를 모르면 권장값 없음.
        assert_eq!(recommended_hold_bars(3.0, 0.0), 0);
    }

    #[test]
    fn bar_sigma_measures_close_to_close_movement() {
        // ±1% 를 번갈아 움직이는 시계열의 σ 는 1% 부근이어야 한다.
        let mut c = bars(50);
        let mut px = 100.0;
        for (i, b) in c.iter_mut().enumerate() {
            px *= if i % 2 == 0 { 1.01 } else { 1.0 / 1.01 };
            b.close = px;
        }
        let s = bar_sigma_pct(&c);
        assert!((0.9..1.1).contains(&s), "σ={s}");
    }

    #[test]
    fn hold_bars_cap_the_reachable_oos_sample_count() {
        // 포지션을 중첩하지 않으므로 신호 상한 = 검정봉수 / 보유봉수.
        let test_bars = oos_test_bars(200); // 10m → 160봉
        assert_eq!(test_bars / 25, 6, "보유 25봉이면 최대 6건 — 최소 10건에 미달");
        assert_eq!(test_bars / 60, 2, "보유 60봉이면 최대 2건 — 구조적으로 통과 불가");
        // 최소 표본 10건을 채우려면 보유봉수가 이 값 이하여야 한다.
        assert_eq!(test_bars / MIN_OOS_SIGNALS as usize, 16);
        // 1m(500봉)은 같은 조건에서 훨씬 여유롭다.
        assert_eq!(oos_test_bars(500) / MIN_OOS_SIGNALS as usize, 40);
    }

    /// A v2 setup reads EMA50, so nearly a whole fold is consumed by warmup.
    /// Walk-forward must hand each fold preceding history — without it a v2
    /// strategy is judged on a handful of bars and looks untradeable for a
    /// reason that has nothing to do with the strategy.
    #[test]
    fn a_fold_alone_leaves_almost_no_bars_after_indicator_warmup() {
        // The dashboard's default 5m fetch is 300 bars → 60-bar folds.
        let (folds, fold_bars) = oos_layout(300);
        assert_eq!((folds, fold_bars), (OOS_FOLDS, 60));
        // Scoring a bare fold would leave 5 usable bars out of 60.
        let usable = fold_bars.saturating_sub(WARMUP_BARS);
        assert!(usable <= 5, "fold {fold_bars} leaves {usable} usable bars");

        // With warmup prepended the whole fold stays scorable. Mirror the
        // arithmetic `evaluate_strategy_live` uses for a mid-series fold.
        let (a, b) = (120usize, 180usize); // one 60-bar fold
        let w = a.saturating_sub(WARMUP_BARS);
        let skip = a - w;
        assert_eq!(skip, WARMUP_BARS);
        assert_eq!((b - w) - skip, b - a, "scored bars must equal the fold length");

        // A fold at the very start of the series clamps instead of underflowing.
        let w0 = 10usize.saturating_sub(WARMUP_BARS);
        assert_eq!(w0, 0);
        assert_eq!(10 - w0, 10, "skip is whatever history exists");
    }

    /// `skip_bars` must exclude the warmup region from scoring, so trades that
    /// belong to an earlier fold are not counted again in this one.
    #[test]
    fn warmup_bars_are_used_for_indicators_but_never_scored() {
        let cfg = crate::strategy::preset("aggressive");
        let cost = CostModel { fee_rate: 0.0, tax_rate: 0.0, slippage_rate: 0.0 };
        let risk = RiskConfig::default();
        // A jagged series so the legacy detectors have something to fire on.
        let mut c = bars(200);
        let mut px = 100.0;
        for (i, b) in c.iter_mut().enumerate() {
            px *= if i % 3 == 0 { 0.985 } else { 1.011 };
            b.open = px / 1.004;
            b.high = px * 1.006;
            b.low = px / 1.008;
            b.close = px;
            b.volume = if i % 5 == 0 { 5000.0 } else { 1000.0 };
        }
        let mtf = MtfContext::neutral(c.len());
        let all = run_strategy_backtest_from(&c, &cfg, Timeframe::M5, &risk, &cost, 10, &mtf, 0);
        let skipped = run_strategy_backtest_from(&c, &cfg, Timeframe::M5, &risk, &cost, 10, &mtf, 150);
        let n = |v: &Value| v["signals"].as_u64().unwrap_or(0);
        // Skipping most of the series can only reduce the scored trade count.
        assert!(n(&skipped) <= n(&all), "{} > {}", n(&skipped), n(&all));
        // Skipping the entire series scores nothing at all.
        let none = run_strategy_backtest_from(&c, &cfg, Timeframe::M5, &risk, &cost, 10, &mtf, c.len());
        assert_eq!(n(&none), 0);
    }
}
