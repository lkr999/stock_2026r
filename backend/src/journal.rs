//! Persistent trade journal — records every closed trade to disk.
//!
//! Builds the paper-trading track record that the live-trading readiness gate
//! (`validation.rs`) evaluates before recommending real orders.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Mutex;

/// One closed-trade record persisted as a JSON object.
#[derive(Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub mode: String, // "paper" | "live"
    pub code: String,
    /// "long" | "short" — P&L in this record is direction-aware.
    #[serde(default)]
    pub side: String,
    pub qty: i64,
    pub entry: f64,
    pub exit: f64,
    pub pnl: f64,
    pub return_pct: f64,
    pub reason: String,
    pub pattern: String,
    pub opened_at: String,
    pub closed_at: String,
}

/// Append-only journal persisted as JSON Lines (one record per line); guarded
/// by a mutex. JSONL keeps `record()` an O(1) append instead of re-reading and
/// pretty-printing the whole file for every trade.
pub struct TradeJournal {
    path: PathBuf,
    lock: Mutex<()>,
}

impl TradeJournal {
    /// Open (or create) the journal at `backend/data/trade_journal.json`.
    /// A legacy JSON-array file is migrated to JSONL in place on first open.
    pub fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/trade_journal.json");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if !path.exists() {
            let _ = std::fs::write(&path, "");
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        if content.trim_start().starts_with('[') {
            if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&content) {
                let jsonl: String = arr.iter().filter_map(|v| serde_json::to_string(v).ok()).map(|l| l + "\n").collect();
                let _ = std::fs::write(&path, jsonl);
                tracing::info!("[journal] migrated legacy JSON array → JSONL ({} records)", arr.len());
            }
        }
        Self { path, lock: Mutex::new(()) }
    }

    fn read(&self) -> Vec<Value> {
        let Ok(s) = std::fs::read_to_string(&self.path) else {
            return vec![];
        };
        // Legacy array (not yet migrated) or JSONL — accept both.
        if s.trim_start().starts_with('[') {
            return serde_json::from_str(&s).unwrap_or_default();
        }
        s.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn write(&self, data: &[Value]) {
        let jsonl: String = data.iter().filter_map(|v| serde_json::to_string(v).ok()).map(|l| l + "\n").collect();
        let _ = std::fs::write(&self.path, jsonl);
    }

    /// Append one closed trade (O(1) — a single line write).
    pub fn record(&self, trade: &TradeRecord) {
        let _g = self.lock.lock().unwrap();
        if let Ok(line) = serde_json::to_string(trade) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
                let _ = writeln!(f, "{line}");
            }
        }
        tracing::info!("[journal] {} {} pnl={:.0} ({:.2}%)", trade.mode, trade.code, trade.pnl, trade.return_pct);
    }

    /// All recorded trades.
    pub fn all(&self) -> Vec<Value> {
        let _g = self.lock.lock().unwrap();
        self.read()
    }

    /// Trades for a single mode (`paper`/`live`).
    pub fn by_mode(&self, mode: &str) -> Vec<Value> {
        self.all().into_iter().filter(|t| t.get("mode").and_then(Value::as_str) == Some(mode)).collect()
    }

    /// Erase the whole journal.
    pub fn clear(&self) {
        let _g = self.lock.lock().unwrap();
        self.write(&[]);
    }

    /// Erase only one mode's records; returns the count removed.
    pub fn clear_mode(&self, mode: &str) -> usize {
        let _g = self.lock.lock().unwrap();
        let data = self.read();
        let kept: Vec<Value> = data
            .iter()
            .filter(|t| t.get("mode").and_then(Value::as_str) != Some(mode))
            .cloned()
            .collect();
        let removed = data.len() - kept.len();
        self.write(&kept);
        removed
    }

    /// Aggregate stats (win rate, total P&L, profit factor, …) for the API.
    pub fn stats(&self, mode: Option<&str>) -> Value {
        let trades = match mode {
            Some(m) => self.by_mode(m),
            None => self.all(),
        };
        if trades.is_empty() {
            return serde_json::json!({
                "count": 0, "win_count": 0, "loss_count": 0, "total_pnl": 0.0,
                "win_rate": 0.0, "profit_factor": Value::Null,
                "avg_return_pct": 0.0, "best_trade_pnl": 0.0, "worst_trade_pnl": 0.0,
            });
        }
        let pnl = |t: &Value| t.get("pnl").and_then(Value::as_f64).unwrap_or(0.0);
        let ret = |t: &Value| t.get("return_pct").and_then(Value::as_f64).unwrap_or(0.0);
        let wins: Vec<&Value> = trades.iter().filter(|t| pnl(t) > 0.0).collect();
        let losses: Vec<&Value> = trades.iter().filter(|t| pnl(t) <= 0.0).collect();
        let gross_profit: f64 = wins.iter().map(|t| pnl(t)).sum();
        let gross_loss: f64 = losses.iter().map(|t| pnl(t)).sum::<f64>().abs();
        let pnls: Vec<f64> = trades.iter().map(pnl).collect();
        let rets: Vec<f64> = trades.iter().map(ret).collect();
        serde_json::json!({
            "count": trades.len(),
            "win_count": wins.len(),
            "loss_count": losses.len(),
            "total_pnl": pnls.iter().sum::<f64>().round(),
            "win_rate": ((wins.len() as f64 / trades.len() as f64) * 1000.0).round() / 10.0,
            "profit_factor": if gross_loss > 0.0 { Value::from((gross_profit / gross_loss * 100.0).round() / 100.0) } else { Value::Null },
            "avg_return_pct": ((rets.iter().sum::<f64>() / rets.len() as f64) * 100.0).round() / 100.0,
            "best_trade_pnl": pnls.iter().cloned().fold(f64::MIN, f64::max).round(),
            "worst_trade_pnl": pnls.iter().cloned().fold(f64::MAX, f64::min).round(),
        })
    }
}

/// Current UTC time as an ISO-8601 string (journal timestamps).
pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}
