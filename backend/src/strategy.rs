//! Strategy mixing/selection config (spec section 10-1).
//!
//! Six signal sources (rule / atr / volume / mtf / ml / gaf) can be toggled and
//! weighted independently, replacing hardcoded composite weights.
//!
//! Strategies come in two **generations** that must never be mixed:
//!
//! * [`Generation::Legacy`] — the original presets. Entry-only: they describe
//!   *what to buy* and inherit whatever risk settings the operator typed into
//!   the form. Kept for backtest comparison.
//! * [`Generation::V2`] — the redesigned presets. Each one additionally ships
//!   its own [`RiskOverrides`] and [`MarketFilter`], because the paper-trading
//!   post-mortem showed the losses came from the *exit mechanics and missing
//!   market context*, not from the entry patterns alone (see the module docs on
//!   [`v2_presets`]).

use crate::risk::RiskConfig;
use serde_json::Value;
use std::collections::HashMap;

/// The six weighted signal sources contributing to the composite score.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Rule,
    Atr,
    Volume,
    Mtf,
    Ml,
    Gaf,
}

impl Source {
    /// Wire name (`"rule"`, `"atr"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Rule => "rule",
            Source::Atr => "atr",
            Source::Volume => "volume",
            Source::Mtf => "mtf",
            Source::Ml => "ml",
            Source::Gaf => "gaf",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "rule" => Source::Rule,
            "atr" => Source::Atr,
            "volume" => Source::Volume,
            "mtf" => Source::Mtf,
            "ml" => Source::Ml,
            "gaf" => Source::Gaf,
            _ => return None,
        })
    }
    /// Iteration order for serializing weights.
    pub fn all() -> [Source; 6] {
        [Source::Rule, Source::Atr, Source::Volume, Source::Mtf, Source::Ml, Source::Gaf]
    }
}

/// Which design generation a strategy belongs to. The UI keeps the two apart so
/// an operator can never accidentally run a legacy preset thinking it carries
/// the v2 protections.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Generation {
    Legacy,
    V2,
}

impl Generation {
    pub fn as_str(self) -> &'static str {
        match self {
            Generation::Legacy => "legacy",
            Generation::V2 => "v2",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "legacy" => Generation::Legacy,
            "v2" => Generation::V2,
            _ => return None,
        })
    }
}

/// Risk settings a strategy imposes on top of the operator's form values.
///
/// Only `Some(..)` fields override. This exists because the biggest realized
/// loss driver was a *risk* setting (a fixed 1.5% stop judged on 10-minute bar
/// closes, so stop-outs filled at −3.4% on average), which no amount of entry
/// tuning can fix. A v2 strategy therefore owns its exit mechanics.
#[derive(Clone, Default)]
pub struct RiskOverrides {
    /// Force ATR-relative stops by clearing the manual fixed-% stop/target.
    /// A fixed percentage smaller than the bar's own noise range is guaranteed
    /// to be gapped through; ATR scales the stop to the instrument's volatility.
    pub force_atr_stops: bool,
    pub stop_loss_atr_mult: Option<f64>,
    pub take_profit_atr_mult: Option<f64>,
    pub trailing_stop_atr: Option<f64>,
    pub use_take_profit: Option<bool>,
    pub use_trailing_stop: Option<bool>,
    /// Evaluate the stop against the *live* price instead of waiting for the
    /// bar close — the single change that keeps realized losses near 1R.
    pub hard_stop_intrabar: Option<bool>,
    pub hard_stop_buffer_pct: Option<f64>,
    pub require_confirmation: Option<bool>,
    pub confirm_window_bars: Option<i64>,
    pub min_hold_bars: Option<i64>,
    pub loss_cooldown_bars: Option<i64>,
    pub reentry_cooldown_bars: Option<i64>,
    pub require_higher_tf_uptrend: Option<bool>,
    pub max_positions: Option<usize>,
}

impl RiskOverrides {
    /// Whether this override set changes anything at all.
    pub fn is_empty(&self) -> bool {
        !self.force_atr_stops
            && self.stop_loss_atr_mult.is_none()
            && self.take_profit_atr_mult.is_none()
            && self.trailing_stop_atr.is_none()
            && self.use_take_profit.is_none()
            && self.use_trailing_stop.is_none()
            && self.hard_stop_intrabar.is_none()
            && self.hard_stop_buffer_pct.is_none()
            && self.require_confirmation.is_none()
            && self.confirm_window_bars.is_none()
            && self.min_hold_bars.is_none()
            && self.loss_cooldown_bars.is_none()
            && self.reentry_cooldown_bars.is_none()
            && self.require_higher_tf_uptrend.is_none()
            && self.max_positions.is_none()
    }

    /// Apply the overrides onto a base config, returning the effective config.
    pub fn apply(&self, base: &RiskConfig) -> RiskConfig {
        let mut c = base.clone();
        if self.force_atr_stops {
            c.stop_loss_pct = 0.0;
            c.take_profit_pct = 0.0;
        }
        macro_rules! set {
            ($($f:ident),* $(,)?) => { $(if let Some(v) = self.$f { c.$f = v; })* };
        }
        set!(
            stop_loss_atr_mult,
            take_profit_atr_mult,
            trailing_stop_atr,
            use_take_profit,
            use_trailing_stop,
            hard_stop_intrabar,
            hard_stop_buffer_pct,
            require_confirmation,
            confirm_window_bars,
            min_hold_bars,
            loss_cooldown_bars,
            reentry_cooldown_bars,
            require_higher_tf_uptrend,
            max_positions,
        );
        c
    }

    /// Human-readable summary rows (label, value) for the UI.
    pub fn describe(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = vec![];
        let mut push = |k: &str, v: String| out.push((k.to_string(), v));
        if self.force_atr_stops {
            push("손절/익절 방식", "ATR 비례 강제 (고정% 무시)".into());
        }
        if let Some(v) = self.stop_loss_atr_mult {
            push("손절", format!("ATR ×{v}"));
        }
        match (self.use_take_profit, self.take_profit_atr_mult) {
            (Some(false), _) => push("익절", "OFF (트레일링으로 수익 확장)".into()),
            (_, Some(v)) => push("익절", format!("ATR ×{v}")),
            _ => {}
        }
        if let Some(v) = self.trailing_stop_atr {
            push("트레일링", format!("ATR ×{v}"));
        }
        if let Some(true) = self.hard_stop_intrabar {
            let buf = self.hard_stop_buffer_pct.unwrap_or(0.0) * 100.0;
            push("실시간 손절", format!("ON (버퍼 {buf:.1}%)"));
        }
        if let Some(v) = self.confirm_window_bars {
            push("확인봉", format!("{v}봉 내"));
        }
        if let Some(v) = self.min_hold_bars {
            push("최소 보유", format!("{v}봉"));
        }
        if let Some(v) = self.loss_cooldown_bars {
            push("손절 후 쿨다운", format!("{v}봉"));
        }
        if let Some(v) = self.max_positions {
            push("최대 보유종목", format!("{v}개"));
        }
        out
    }
}

/// Default market proxy: KODEX 200 — a normal listed ETF, so it comes through
/// the same t8452 minute-candle path as any stock (no index-specific API).
pub const DEFAULT_MARKET_PROXY: &str = "069500";

/// Market-context entry gate: trade only when the broad market allows it, and
/// only in names that are outperforming it.
///
/// This is the direct answer to "the index rises and we still lose": nothing in
/// the legacy path ever looked at the market at all, so entries kept landing in
/// laggards while the proxy rallied.
#[derive(Clone)]
pub struct MarketFilter {
    pub enabled: bool,
    /// Listed proxy for the broad market (an ETF code, not an index code).
    pub proxy_code: String,
    /// Block entries while the proxy is below its EMA20 / trending down.
    pub require_risk_on: bool,
    /// Minimum relative strength: (stock % change − proxy % change) over
    /// `rs_lookback` closed bars, in percentage points. 0 = merely keep pace.
    pub min_rs: f64,
    pub rs_lookback: usize,
}

impl Default for MarketFilter {
    fn default() -> Self {
        Self {
            enabled: false,
            proxy_code: DEFAULT_MARKET_PROXY.into(),
            require_risk_on: false,
            min_rs: 0.0,
            rs_lookback: 20,
        }
    }
}

impl MarketFilter {
    fn on(require_risk_on: bool, min_rs: f64) -> Self {
        Self { enabled: true, require_risk_on, min_rs, ..Default::default() }
    }
    fn to_json(&self) -> Value {
        serde_json::json!({
            "enabled": self.enabled,
            "proxy_code": self.proxy_code,
            "require_risk_on": self.require_risk_on,
            "min_rs": self.min_rs,
            "rs_lookback": self.rs_lookback,
        })
    }
}

/// A tunable strategy: source weights + enabled patterns + entry gates.
#[derive(Clone)]
pub struct StrategyConfig {
    pub name: String,
    pub weights: HashMap<Source, f64>,
    pub enabled_patterns: Vec<String>,
    pub entry_threshold: f64,
    pub require_volume_confirm: bool,
    pub min_reward_risk: f64,
    pub min_edge_over_cost: f64,
    /// Timeframe this strategy is tuned for (used by the "auto" backtest mode).
    pub recommended_tf: String,
    /// Design generation — legacy presets and v2 presets are selected separately.
    pub generation: Generation,
    /// One-line description shown in the strategy picker.
    pub summary: String,
    /// Risk settings this strategy imposes (v2 only; empty for legacy).
    pub risk_overrides: RiskOverrides,
    /// Market-context gate (v2 only; disabled for legacy).
    pub market_filter: MarketFilter,
}

fn default_weights() -> HashMap<Source, f64> {
    HashMap::from([
        (Source::Rule, 0.40),
        (Source::Atr, 0.15),
        (Source::Volume, 0.15),
        (Source::Mtf, 0.30),
        (Source::Ml, 0.00),
        (Source::Gaf, 0.00),
    ])
}

fn default_patterns() -> Vec<String> {
    ["three_white_soldiers", "morning_star", "bullish_engulfing", "hammer"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            name: "balanced".into(),
            weights: default_weights(),
            enabled_patterns: default_patterns(),
            entry_threshold: 0.65,
            require_volume_confirm: false,
            min_reward_risk: 0.0,
            min_edge_over_cost: 0.0,
            recommended_tf: "5m".into(),
            generation: Generation::Legacy,
            summary: String::new(),
            risk_overrides: RiskOverrides::default(),
            market_filter: MarketFilter::default(),
        }
    }
}

impl StrategyConfig {
    /// Weighted average of *computed* sources, renormalized.
    ///
    /// A source contributes only when its weight > 0 **and** its signal was
    /// actually computed (`Some`). Sources not computed this scan (e.g. MTF/ML)
    /// are excluded from the denominator too — otherwise their weight would drag
    /// every score below the threshold.
    pub fn composite(&self, signals: &HashMap<Source, Option<f64>>) -> f64 {
        let active: Vec<(f64, f64)> = self
            .weights
            .iter()
            .filter_map(|(s, &w)| match signals.get(s) {
                Some(Some(v)) if w > 0.0 => Some((*v, w)),
                _ => None,
            })
            .collect();
        let total_w: f64 = active.iter().map(|(_, w)| w).sum::<f64>().max(1e-9);
        active.iter().map(|(v, w)| v * w).sum::<f64>() / total_w
    }

    /// The timeframe this strategy is tuned for, parsed (defaults to 5m).
    pub fn recommended_tf(&self) -> crate::timeframe::Timeframe {
        crate::timeframe::Timeframe::parse(&self.recommended_tf).unwrap_or(crate::timeframe::Timeframe::M5)
    }

    /// The risk config this strategy actually trades with: the operator's base
    /// settings with the strategy's own overrides applied on top.
    pub fn effective_risk(&self, base: &RiskConfig) -> RiskConfig {
        self.risk_overrides.apply(base)
    }

    /// Full serialization for the strategy picker.
    pub fn to_json(&self) -> Value {
        let weights: serde_json::Map<String, Value> = Source::all()
            .iter()
            .map(|s| (s.as_str().to_string(), serde_json::json!(self.weights.get(s).copied().unwrap_or(0.0))))
            .collect();
        serde_json::json!({
            "name": self.name,
            "generation": self.generation.as_str(),
            "summary": self.summary,
            "weights": weights,
            "enabled_patterns": self.enabled_patterns,
            "entry_threshold": self.entry_threshold,
            "require_volume_confirm": self.require_volume_confirm,
            "min_reward_risk": self.min_reward_risk,
            "min_edge_over_cost": self.min_edge_over_cost,
            "recommended_tf": self.recommended_tf,
            "market_filter": self.market_filter.to_json(),
            "risk_overrides": self
                .risk_overrides
                .describe()
                .into_iter()
                .map(|(k, v)| serde_json::json!({"label": k, "value": v}))
                .collect::<Vec<_>>(),
            "has_risk_overrides": !self.risk_overrides.is_empty(),
        })
    }
}

/// The four built-in presets (conservative / balanced / aggressive / ml_blended).
pub fn presets() -> Vec<StrategyConfig> {
    let w = |r, a, v, m, ml, g: f64| {
        HashMap::from([
            (Source::Rule, r),
            (Source::Atr, a),
            (Source::Volume, v),
            (Source::Mtf, m),
            (Source::Ml, ml),
            (Source::Gaf, g),
        ])
    };
    vec![
        StrategyConfig {
            name: "conservative".into(),
            weights: w(0.35, 0.15, 0.20, 0.30, 0.0, 0.0),
            entry_threshold: 0.75,
            require_volume_confirm: true,
            min_reward_risk: 1.5,
            min_edge_over_cost: 3.0,
            ..Default::default()
        },
        StrategyConfig {
            name: "balanced".into(),
            require_volume_confirm: true,
            min_reward_risk: 1.3,
            min_edge_over_cost: 2.0,
            ..Default::default()
        },
        StrategyConfig {
            name: "aggressive".into(),
            weights: w(0.60, 0.10, 0.30, 0.0, 0.0, 0.0),
            entry_threshold: 0.55,
            ..Default::default()
        },
        StrategyConfig {
            name: "ml_blended".into(),
            weights: w(0.25, 0.10, 0.10, 0.20, 0.35, 0.0),
            entry_threshold: 0.62,
            ..Default::default()
        },
        // -------- day-trading (단타) setups: VWAP/ORB/EMA driven (매수 진입 전용) --------
        StrategyConfig {
            name: "vwap_scalp".into(),
            recommended_tf: "1m".into(),
            weights: w(0.55, 0.15, 0.30, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["vwap_reclaim", "vwap_bounce"]),
            entry_threshold: 0.60,
            require_volume_confirm: true,
            min_reward_risk: 1.2,
            ..Default::default()
        },
        StrategyConfig {
            name: "orb_breakout".into(),
            recommended_tf: "1m".into(),
            weights: w(0.50, 0.15, 0.35, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["orb_breakout", "marubozu_bull"]),
            entry_threshold: 0.60,
            require_volume_confirm: true,
            min_reward_risk: 1.3,
            ..Default::default()
        },
        StrategyConfig {
            name: "ema_pullback".into(),
            recommended_tf: "1m".into(),
            weights: w(0.60, 0.20, 0.20, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["ema_pullback", "pin_bar_bull"]),
            entry_threshold: 0.58,
            min_reward_risk: 1.2,
            ..Default::default()
        },
        StrategyConfig {
            name: "intraday_blended".into(),
            recommended_tf: "1m".into(),
            weights: w(0.45, 0.15, 0.25, 0.15, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "vwap_reclaim", "vwap_bounce",
                "orb_breakout", "ema_pullback",
                "pin_bar_bull",
                "rsi_oversold_bounce",
                "bb_squeeze_break_up",
            ]),
            entry_threshold: 0.60,
            require_volume_confirm: true,
            min_reward_risk: 1.2,
            ..Default::default()
        },
        // RSI(14) 과매도 반등 — 추격 대신 소진 지점을 사는 평균회귀형(Connors
        // 계열)이라 역사적으로 승률이 높은 부류의 셋업. 반전 캔들(해머/핀바/
        // 트위저)을 함께 켜 바닥 확인 신호를 보강한다.
        StrategyConfig {
            name: "rsi_meanrev".into(),
            recommended_tf: "5m".into(),
            weights: w(0.60, 0.15, 0.25, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "rsi_oversold_bounce",
                "hammer", "pin_bar_bull",
                "tweezer_bottom",
            ]),
            entry_threshold: 0.58,
            min_reward_risk: 1.2,
            ..Default::default()
        },
        // 볼린저 스퀴즈 돌파 — 밴드폭이 20봉 최저 부근까지 수축한 뒤 상단 밴드
        // 밖 종가로 해소될 때 진입 (변동성 수축→확장의 초입을 탄다). 인사이드바
        // 돌파·마루보주를 함께 켜 돌파 임펄스 신호를 보강한다.
        StrategyConfig {
            name: "bb_squeeze".into(),
            recommended_tf: "5m".into(),
            weights: w(0.50, 0.15, 0.35, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "bb_squeeze_break_up",
                "inside_bar_break_up",
                "marubozu_bull",
            ]),
            entry_threshold: 0.60,
            require_volume_confirm: true,
            min_reward_risk: 1.3,
            ..Default::default()
        },
    ]
}

/// The redesigned (v2) presets.
///
/// Every one of them is built around the five failures the paper record exposed
/// over 130 trades (43% win rate, +1.92% average win against a −3.29% average
/// loss — a 0.58 payoff, i.e. a −1.0%/trade expectancy):
///
/// 1. **Stops filled far past their level.** A fixed 1.5% stop judged only on
///    10-minute bar closes was routinely gapped through; the 71 stop-outs
///    averaged −3.42%, 2.3× their budget. → every v2 preset forces ATR-relative
///    stops (`force_atr_stops`) *and* intrabar evaluation (`hard_stop_intrabar`)
///    so a 1R stop actually costs about 1R.
/// 2. **Reward:risk below 1 in practice.** → `min_reward_risk` ≥ 1.8 on every
///    preset, and the take-profit multiple is always ≥ 2× the stop multiple.
/// 3. **No market context at all.** The legacy higher-TF gate only inspected the
///    symbol itself, so a rising index bought laggards. → `market_filter` gates
///    on the proxy's own regime *and* on the symbol's relative strength.
/// 4. **Entry gates that filtered almost nothing.** → higher `entry_threshold`
///    plus setups that carry their quality tests inside the detector.
/// 5. **No selectivity or cooldown discipline.** → tighter `max_positions` and
///    much longer `loss_cooldown_bars` to stop re-entering a losing name.
pub fn v2_presets() -> Vec<StrategyConfig> {
    let w = |r, a, v, m, ml, g: f64| {
        HashMap::from([
            (Source::Rule, r),
            (Source::Atr, a),
            (Source::Volume, v),
            (Source::Mtf, m),
            (Source::Ml, ml),
            (Source::Gaf, g),
        ])
    };
    // Shared v2 exit doctrine — ATR stops, evaluated live, never fixed-%.
    let base_risk = || RiskOverrides {
        force_atr_stops: true,
        hard_stop_intrabar: Some(true),
        hard_stop_buffer_pct: Some(0.001),
        require_confirmation: Some(true),
        ..Default::default()
    };

    vec![
        // ---- 추세 순응 러너 -------------------------------------------------
        // 익절 상한을 없애 이익을 끝까지 끌고 간다. 평균이익 < 평균손실 구조를
        // 뒤집는 가장 직접적인 수단 — 손실은 1R로 고정하고 이익만 열어둔다.
        StrategyConfig {
            name: "v2_trend_rider".into(),
            generation: Generation::V2,
            summary: "추세 순응 · 러너형 — 손실은 1R 고정, 익절 상한 없이 ATR 트레일링으로 이익을 끝까지 확장".into(),
            recommended_tf: "5m".into(),
            weights: w(0.55, 0.15, 0.30, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["v2_trend_pullback", "v2_power_breakout"]),
            entry_threshold: 0.68,
            require_volume_confirm: true,
            min_reward_risk: 1.8,
            min_edge_over_cost: 3.0,
            risk_overrides: RiskOverrides {
                stop_loss_atr_mult: Some(1.6),
                take_profit_atr_mult: Some(4.0),
                use_take_profit: Some(false), // 러너: 목표가로 끊지 않는다
                use_trailing_stop: Some(true),
                trailing_stop_atr: Some(2.2),
                confirm_window_bars: Some(2),
                min_hold_bars: Some(2),
                loss_cooldown_bars: Some(12),
                reentry_cooldown_bars: Some(4),
                max_positions: Some(6),
                ..base_risk()
            },
            market_filter: MarketFilter::on(true, 0.0),
            ..Default::default()
        },
        // ---- 정밀 돌파 -------------------------------------------------------
        // 기존 orb_breakout(승률 37.5%)의 대체. 거래량·몸통·마감위치를 디텍터
        // 안에서 강제하고, 맨몸 돌파 대신 되돌림 후 지지(리테스트)까지 본다.
        StrategyConfig {
            name: "v2_breakout_pro".into(),
            generation: Generation::V2,
            summary: "정밀 돌파 — 거래량 2배·강한 몸통·상단 마감을 통과한 돌파와 오프닝레인지 리테스트만 진입 (RR 2.0 고정)".into(),
            recommended_tf: "5m".into(),
            weights: w(0.50, 0.15, 0.35, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "v2_power_breakout",
                "v2_orb_retest",
                "v2_squeeze_expansion",
            ]),
            entry_threshold: 0.70,
            require_volume_confirm: true,
            min_reward_risk: 2.0,
            min_edge_over_cost: 3.0,
            risk_overrides: RiskOverrides {
                stop_loss_atr_mult: Some(1.4),
                // 3.0/1.4 = 2.14R. Sitting *exactly* on `min_reward_risk` would
                // be fatal here: 2.8/1.4 evaluates to 1.9999999999999998 in
                // f64, so the RR gate would reject every single entry.
                take_profit_atr_mult: Some(3.0),
                use_take_profit: Some(true),
                use_trailing_stop: Some(true),
                trailing_stop_atr: Some(2.0),
                confirm_window_bars: Some(2),
                min_hold_bars: Some(1),
                loss_cooldown_bars: Some(15),
                reentry_cooldown_bars: Some(4),
                max_positions: Some(5),
                ..base_risk()
            },
            // 돌파는 시장이 받쳐줘야 산다 — 시장 프록시 대비 초과수익 종목만.
            market_filter: MarketFilter::on(true, 0.5),
            ..Default::default()
        },
        // ---- 눌림목 저격 -----------------------------------------------------
        // 기존 ema_pullback_long(승률 20%, -25,595원)의 대체. 결정적 차이는
        // "추세 정렬 확인 후 되돌림을 산다"는 것 — 고점 추격이 아니다.
        StrategyConfig {
            name: "v2_pullback_sniper".into(),
            generation: Generation::V2,
            summary: "눌림목 저격 — EMA20>EMA50 정렬 확인 후 EMA20 되돌림 지지에서만 진입 (고점 추격 금지)".into(),
            recommended_tf: "5m".into(),
            weights: w(0.60, 0.20, 0.20, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["v2_trend_pullback", "v2_rsi_reversion"]),
            entry_threshold: 0.66,
            require_volume_confirm: false, // 눌림목은 거래량이 마르는 게 정상
            min_reward_risk: 1.8,
            min_edge_over_cost: 2.5,
            risk_overrides: RiskOverrides {
                stop_loss_atr_mult: Some(1.2),
                take_profit_atr_mult: Some(2.4),
                use_take_profit: Some(true),
                use_trailing_stop: Some(true),
                trailing_stop_atr: Some(1.8),
                confirm_window_bars: Some(3),
                min_hold_bars: Some(2),
                loss_cooldown_bars: Some(10),
                reentry_cooldown_bars: Some(3),
                max_positions: Some(5),
                ..base_risk()
            },
            market_filter: MarketFilter::on(true, 0.0),
            ..Default::default()
        },
        // ---- 과매도 반등 (시장 가드) ------------------------------------------
        // 평균회귀는 원래 승률이 높은 부류지만, 하락장에서 쓰면 낙하하는 칼을
        // 잡는다. 상승 추세(EMA50 위)에서만 허용하고 시장 리스크온을 요구한다.
        StrategyConfig {
            name: "v2_meanrev_guard".into(),
            generation: Generation::V2,
            summary: "과매도 반등 · 시장 가드 — 상승 추세(EMA50 위) + 시장 리스크온일 때만 RSI 소진 반등을 매수 (고승률·소폭 익절)".into(),
            recommended_tf: "5m".into(),
            weights: w(0.65, 0.15, 0.20, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["v2_rsi_reversion"]),
            entry_threshold: 0.64,
            require_volume_confirm: false,
            min_reward_risk: 1.8,
            min_edge_over_cost: 2.5,
            risk_overrides: RiskOverrides {
                stop_loss_atr_mult: Some(1.1),
                take_profit_atr_mult: Some(2.2), // 2.0R
                use_take_profit: Some(true),
                use_trailing_stop: Some(false), // 짧은 반등을 트레일링이 조기 종료시킨다
                confirm_window_bars: Some(2),
                min_hold_bars: Some(1),
                loss_cooldown_bars: Some(10),
                reentry_cooldown_bars: Some(3),
                max_positions: Some(4),
                ..base_risk()
            },
            market_filter: MarketFilter::on(true, -0.5), // 약간의 언더퍼폼은 허용(반등 대상이므로)
            ..Default::default()
        },
        // ---- 전천후 블렌드 ----------------------------------------------------
        StrategyConfig {
            name: "v2_allweather".into(),
            generation: Generation::V2,
            summary: "전천후 블렌드 — v2 셋업 전체를 켜고 게이트를 중간 강도로 (신호 빈도와 품질의 절충안)".into(),
            recommended_tf: "5m".into(),
            weights: w(0.50, 0.15, 0.25, 0.10, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "v2_trend_pullback",
                "v2_power_breakout",
                "v2_orb_retest",
                "v2_rsi_reversion",
                "v2_squeeze_expansion",
            ]),
            entry_threshold: 0.66,
            require_volume_confirm: false,
            min_reward_risk: 1.8,
            min_edge_over_cost: 2.5,
            risk_overrides: RiskOverrides {
                stop_loss_atr_mult: Some(1.5),
                take_profit_atr_mult: Some(3.0),
                use_take_profit: Some(true),
                use_trailing_stop: Some(true),
                trailing_stop_atr: Some(2.0),
                confirm_window_bars: Some(2),
                min_hold_bars: Some(1),
                loss_cooldown_bars: Some(12),
                reentry_cooldown_bars: Some(3),
                max_positions: Some(6),
                ..base_risk()
            },
            market_filter: MarketFilter::on(true, 0.0),
            ..Default::default()
        },
    ]
}

/// Every preset of both generations (legacy first, then v2).
pub fn all_presets() -> Vec<StrategyConfig> {
    let mut v = presets();
    v.extend(v2_presets());
    v
}

/// Presets belonging to one generation.
pub fn presets_of(gen: Generation) -> Vec<StrategyConfig> {
    match gen {
        Generation::Legacy => presets(),
        Generation::V2 => v2_presets(),
    }
}

/// Helper to build an `enabled_patterns` list from string slices.
fn setup_patterns(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// Look up a preset by name across both generations, defaulting to `balanced`.
pub fn preset(name: &str) -> StrategyConfig {
    all_presets()
        .into_iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| presets().into_iter().find(|p| p.name == "balanced").unwrap())
}

/// Resolve a preset name, or a JSON object overriding a base preset, into a config.
pub fn resolve(value: &Value) -> StrategyConfig {
    match value {
        Value::String(name) => preset(name),
        Value::Object(map) => {
            let base = preset(map.get("name").and_then(Value::as_str).unwrap_or("balanced"));
            let mut weights = base.weights.clone();
            if let Some(Value::Object(wm)) = map.get("weights") {
                for (k, v) in wm {
                    if let (Some(src), Some(f)) = (Source::parse(k), v.as_f64()) {
                        weights.insert(src, f);
                    }
                }
            }
            let f = |k: &str, d: f64| map.get(k).and_then(Value::as_f64).unwrap_or(d);
            let bo = |k: &str, d: bool| map.get(k).and_then(Value::as_bool).unwrap_or(d);
            // The generation, its risk doctrine and its market gate ride along
            // with the base preset — a caller tweaking weights must not be able
            // to silently strip a v2 strategy of the protections that define it.
            // Only the market filter's tunable numbers may be adjusted.
            let mut market_filter = base.market_filter.clone();
            if let Some(Value::Object(mf)) = map.get("market_filter") {
                let g = |k: &str| mf.get(k);
                if let Some(v) = g("proxy_code").and_then(Value::as_str) {
                    if !v.trim().is_empty() {
                        market_filter.proxy_code = v.trim().to_string();
                    }
                }
                if let Some(v) = g("require_risk_on").and_then(Value::as_bool) {
                    market_filter.require_risk_on = v;
                }
                if let Some(v) = g("min_rs").and_then(Value::as_f64) {
                    market_filter.min_rs = v;
                }
                if let Some(v) = g("rs_lookback").and_then(Value::as_u64) {
                    market_filter.rs_lookback = (v as usize).clamp(5, 120);
                }
            }
            StrategyConfig {
                name: map.get("name").and_then(Value::as_str).unwrap_or(&base.name).to_string(),
                weights,
                enabled_patterns: map
                    .get("enabled_patterns")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_else(|| base.enabled_patterns.clone()),
                entry_threshold: f("entry_threshold", base.entry_threshold),
                require_volume_confirm: bo("require_volume_confirm", base.require_volume_confirm),
                min_reward_risk: f("min_reward_risk", base.min_reward_risk),
                min_edge_over_cost: f("min_edge_over_cost", base.min_edge_over_cost),
                recommended_tf: map.get("recommended_tf").and_then(Value::as_str).unwrap_or(&base.recommended_tf).to_string(),
                generation: map
                    .get("generation")
                    .and_then(Value::as_str)
                    .and_then(Generation::parse)
                    .unwrap_or(base.generation),
                summary: base.summary.clone(),
                risk_overrides: base.risk_overrides.clone(),
                market_filter,
            }
        }
        _ => preset("balanced"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_presets_all_carry_the_exit_doctrine() {
        for p in v2_presets() {
            assert_eq!(p.generation, Generation::V2, "{}", p.name);
            let o = &p.risk_overrides;
            // The fixed-% stop is what produced −3.4% stop-outs; v2 must clear it.
            assert!(o.force_atr_stops, "{} must force ATR stops", p.name);
            assert_eq!(o.hard_stop_intrabar, Some(true), "{} must stop intrabar", p.name);
            assert!(p.min_reward_risk >= 1.8, "{}", p.name);
            assert!(p.market_filter.enabled, "{}", p.name);
            // The strategy's own ATR multiples must clear its own reward/risk
            // gate with room to spare. Landing exactly on the threshold is a
            // silent kill switch — f64 division can put the ratio a hair below
            // it (2.8/1.4 == 1.9999999999999998), rejecting every entry.
            let (sl, tp) = (o.stop_loss_atr_mult.unwrap(), o.take_profit_atr_mult.unwrap());
            let rr = tp / sl;
            assert!(
                rr >= p.min_reward_risk * 1.01,
                "{}: ATR multiples give RR {rr:.4}, too close to its own min_reward_risk {}",
                p.name,
                p.min_reward_risk
            );
        }
    }

    #[test]
    fn overrides_replace_the_fixed_pct_stop_with_atr() {
        let base = RiskConfig { stop_loss_pct: 0.015, take_profit_pct: 0.02, ..Default::default() };
        let eff = preset("v2_breakout_pro").effective_risk(&base);
        assert_eq!((eff.stop_loss_pct, eff.take_profit_pct), (0.0, 0.0));
        assert_eq!(eff.stop_loss_atr_mult, 1.4);
        assert!(eff.hard_stop_intrabar);
    }

    #[test]
    fn resolve_cannot_strip_v2_protections_via_weight_tweaks() {
        let v = serde_json::json!({"name": "v2_trend_rider", "entry_threshold": 0.5});
        let cfg = resolve(&v);
        assert_eq!(cfg.generation, Generation::V2);
        assert!(cfg.risk_overrides.force_atr_stops);
        assert!(cfg.market_filter.enabled);
        assert_eq!(cfg.entry_threshold, 0.5); // tunables still apply
    }

    #[test]
    fn legacy_presets_impose_no_risk_overrides() {
        for p in presets() {
            assert_eq!(p.generation, Generation::Legacy, "{}", p.name);
            assert!(p.risk_overrides.is_empty(), "{}", p.name);
            assert!(!p.market_filter.enabled, "{}", p.name);
        }
    }
}
