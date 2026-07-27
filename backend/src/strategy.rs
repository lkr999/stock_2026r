//! Strategy mixing/selection config (spec section 10-1).
//!
//! Six signal sources (rule / atr / volume / mtf / ml / gaf) can be toggled and
//! weighted independently, replacing hardcoded composite weights.

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

/// A tunable strategy: source weights + enabled patterns + entry gates.
#[derive(Clone)]
pub struct StrategyConfig {
    pub name: String,
    pub weights: HashMap<Source, f64>,
    pub enabled_patterns: Vec<String>,
    pub entry_threshold: f64,
    pub direction: String,
    pub require_volume_confirm: bool,
    pub min_reward_risk: f64,
    pub min_edge_over_cost: f64,
    /// Timeframe this strategy is tuned for (used by the "auto" backtest mode).
    pub recommended_tf: String,
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
            direction: "long_only".into(),
            require_volume_confirm: false,
            min_reward_risk: 0.0,
            min_edge_over_cost: 0.0,
            recommended_tf: "5m".into(),
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

    /// Whether long entries are permitted by `direction`.
    pub fn allows_long(&self) -> bool {
        self.direction != "short_only"
    }

    /// Whether short entries are permitted by `direction` (`both` / `short_only`).
    pub fn allows_short(&self) -> bool {
        self.direction == "both" || self.direction == "short_only"
    }

    /// The timeframe this strategy is tuned for, parsed (defaults to 5m).
    pub fn recommended_tf(&self) -> crate::timeframe::Timeframe {
        crate::timeframe::Timeframe::parse(&self.recommended_tf).unwrap_or(crate::timeframe::Timeframe::M5)
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
        // -------- day-trading (단타) setups: long+short, VWAP/ORB/EMA driven --------
        StrategyConfig {
            name: "vwap_scalp".into(),
            recommended_tf: "1m".into(),
            weights: w(0.55, 0.15, 0.30, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["vwap_reclaim", "vwap_bounce", "vwap_loss", "vwap_reject"]),
            entry_threshold: 0.60,
            direction: "both".into(),
            require_volume_confirm: true,
            min_reward_risk: 1.2,
            ..Default::default()
        },
        StrategyConfig {
            name: "orb_breakout".into(),
            recommended_tf: "1m".into(),
            weights: w(0.50, 0.15, 0.35, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["orb_breakout", "orb_breakdown", "marubozu_bull", "marubozu_bear"]),
            entry_threshold: 0.60,
            direction: "both".into(),
            require_volume_confirm: true,
            min_reward_risk: 1.3,
            ..Default::default()
        },
        StrategyConfig {
            name: "ema_pullback".into(),
            recommended_tf: "1m".into(),
            weights: w(0.60, 0.20, 0.20, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&["ema_pullback_long", "ema_pullback_short", "pin_bar_bull", "pin_bar_bear"]),
            entry_threshold: 0.58,
            direction: "both".into(),
            min_reward_risk: 1.2,
            ..Default::default()
        },
        StrategyConfig {
            name: "intraday_blended".into(),
            recommended_tf: "1m".into(),
            weights: w(0.45, 0.15, 0.25, 0.15, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "vwap_reclaim", "vwap_bounce", "vwap_loss", "vwap_reject",
                "orb_breakout", "orb_breakdown", "ema_pullback_long", "ema_pullback_short",
                "pin_bar_bull", "pin_bar_bear",
                "rsi_oversold_bounce", "rsi_overbought_drop",
                "bb_squeeze_break_up", "bb_squeeze_break_down",
            ]),
            entry_threshold: 0.60,
            direction: "both".into(),
            require_volume_confirm: true,
            min_reward_risk: 1.2,
            ..Default::default()
        },
        // RSI(14) 과매도 반등 / 과매수 하락 — 추격 대신 소진 지점을 사는
        // 평균회귀형(Connors 계열)이라 역사적으로 승률이 높은 부류의 셋업.
        // 반전 캔들(해머/핀바/트위저)을 함께 켜 바닥 확인 신호를 보강한다.
        StrategyConfig {
            name: "rsi_meanrev".into(),
            recommended_tf: "5m".into(),
            weights: w(0.60, 0.15, 0.25, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "rsi_oversold_bounce", "rsi_overbought_drop",
                "hammer", "pin_bar_bull", "pin_bar_bear",
                "tweezer_bottom", "tweezer_top",
            ]),
            entry_threshold: 0.58,
            direction: "both".into(),
            min_reward_risk: 1.2,
            ..Default::default()
        },
        // 볼린저 스퀴즈 돌파 — 밴드폭이 20봉 최저 부근까지 수축한 뒤 밴드 밖
        // 종가로 해소될 때 진입 (변동성 수축→확장의 초입을 탄다). 인사이드바
        // 돌파·마루보주를 함께 켜 돌파 임펄스 신호를 보강한다.
        StrategyConfig {
            name: "bb_squeeze".into(),
            recommended_tf: "5m".into(),
            weights: w(0.50, 0.15, 0.35, 0.0, 0.0, 0.0),
            enabled_patterns: setup_patterns(&[
                "bb_squeeze_break_up", "bb_squeeze_break_down",
                "inside_bar_break_up", "inside_bar_break_down",
                "marubozu_bull", "marubozu_bear",
            ]),
            entry_threshold: 0.60,
            direction: "both".into(),
            require_volume_confirm: true,
            min_reward_risk: 1.3,
            ..Default::default()
        },
    ]
}

/// Helper to build an `enabled_patterns` list from string slices.
fn setup_patterns(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// Look up a preset by name, defaulting to `balanced`.
pub fn preset(name: &str) -> StrategyConfig {
    presets()
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
            StrategyConfig {
                name: map.get("name").and_then(Value::as_str).unwrap_or(&base.name).to_string(),
                weights,
                enabled_patterns: map
                    .get("enabled_patterns")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_else(|| base.enabled_patterns.clone()),
                entry_threshold: f("entry_threshold", base.entry_threshold),
                direction: map.get("direction").and_then(Value::as_str).unwrap_or(&base.direction).to_string(),
                require_volume_confirm: bo("require_volume_confirm", base.require_volume_confirm),
                min_reward_risk: f("min_reward_risk", base.min_reward_risk),
                min_edge_over_cost: f("min_edge_over_cost", base.min_edge_over_cost),
                recommended_tf: map.get("recommended_tf").and_then(Value::as_str).unwrap_or(&base.recommended_tf).to_string(),
            }
        }
        _ => preset("balanced"),
    }
}
