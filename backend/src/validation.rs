//! Live-trading readiness gate (spec section 11-2 — 실전 투입 체크리스트).
//!
//! Evaluates the persisted paper-trading record against objective criteria. The
//! report is advisory only — it never blocks live trading.

use crate::journal::TradeJournal;
use chrono::DateTime;
use serde_json::{json, Value};

/// Objective thresholds a paper track record should meet before going live.
pub struct ReadinessCriteria {
    pub min_paper_trades: usize,
    pub min_paper_days: i64,
    pub min_win_rate: f64,
    pub min_profit_factor: f64,
    pub require_positive_pnl: bool,
}

/// Aggregate paper-trading statistics used by the readiness checks.
fn paper_stats(trades: &[Value]) -> Value {
    if trades.is_empty() {
        return json!({"trades": 0, "win_rate": 0.0, "profit_factor": 0.0,
                      "total_pnl": 0.0, "avg_return_pct": 0.0, "span_days": 0.0});
    }
    let f = |t: &Value, k: &str| t.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let rets: Vec<f64> = trades.iter().map(|t| f(t, "return_pct")).collect();
    let pnls: Vec<f64> = trades.iter().map(|t| f(t, "pnl")).collect();
    let win_sum: f64 = rets.iter().filter(|&&r| r > 0.0).sum();
    let loss_sum: f64 = rets.iter().filter(|&&r| r <= 0.0).sum();
    let pf = if loss_sum != 0.0 {
        win_sum / loss_sum.abs()
    } else if win_sum > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };
    // Span = days between the earliest and latest closed_at timestamps.
    let times: Vec<i64> = trades
        .iter()
        .filter_map(|t| t.get("closed_at").and_then(Value::as_str))
        .filter_map(|s| DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp()))
        .collect();
    let span_days = if times.len() >= 2 {
        (times.iter().max().unwrap() - times.iter().min().unwrap()) as f64 / 86400.0
    } else {
        0.0
    };
    let wins = rets.iter().filter(|&&r| r > 0.0).count();
    json!({
        "trades": trades.len(),
        "win_rate": wins as f64 / rets.len() as f64,
        "profit_factor": pf,
        "total_pnl": pnls.iter().sum::<f64>(),
        "avg_return_pct": rets.iter().sum::<f64>() / rets.len() as f64,
        "span_days": (span_days * 100.0).round() / 100.0,
    })
}

/// Build one criterion check object for the report.
fn criterion(key: &str, label: &str, passed: bool, actual: f64, required: f64) -> Value {
    json!({"key": key, "label": label, "passed": passed, "actual": actual, "required": required})
}

/// Evaluate the paper record and return `{ready, criteria, stats}`.
pub fn evaluate(journal: &TradeJournal, c: &ReadinessCriteria) -> Value {
    let trades = journal.by_mode("paper");
    let s = paper_stats(&trades);
    let g = |k: &str| s.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let pf = g("profit_factor");

    let mut checks = vec![
        criterion("trades", "페이퍼 거래 수", g("trades") >= c.min_paper_trades as f64, g("trades"), c.min_paper_trades as f64),
        criterion("days", "검증 기간(일)", g("span_days") >= c.min_paper_days as f64, g("span_days"), c.min_paper_days as f64),
        criterion("win_rate", "승률", g("win_rate") >= c.min_win_rate, (g("win_rate") * 1000.0).round() / 1000.0, c.min_win_rate),
        criterion(
            "profit_factor",
            "Profit Factor",
            pf >= c.min_profit_factor,
            if pf.is_infinite() { 999.0 } else { (pf * 100.0).round() / 100.0 },
            c.min_profit_factor,
        ),
    ];
    if c.require_positive_pnl {
        checks.push(criterion("pnl", "누적 손익(>0)", g("total_pnl") > 0.0, g("total_pnl").round(), 0.0));
    }
    let ready = checks.iter().all(|c| c.get("passed").and_then(Value::as_bool).unwrap_or(false));
    json!({"ready": ready, "criteria": checks, "stats": s})
}
