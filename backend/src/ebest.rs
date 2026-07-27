//! eBest broker REST API client (the *only* market-data source — no synthetic data).
//!
//! Handles OAuth token caching, per-TR-code rate limiting (~1 call/sec), and an
//! automatic one-shot retry when the server reports an expired/invalid token.

use crate::config::Settings;
use chrono::Utc;
use chrono_tz::Asia::Seoul;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::sleep;

const MIN_CALL_INTERVAL: Duration = Duration::from_millis(1100); // ~1 call/sec per TR code
const TOKEN_ERROR_CODES: [&str; 3] = ["IGW00123", "IGW00124", "IGW00125"];
const TOKEN_ERROR_KEYWORDS: [&str; 5] = [
    "유효하지 않은 token",
    "token이 없습니다",
    "token 만료",
    "invalid token",
    "unauthorized",
];
const RATE_LIMIT_CODE: &str = "IGW00201";

/// Parse an eBest numeric field (handles strings with commas, null, NaN).
pub fn parse_float(v: &Value) -> Option<f64> {
    match v {
        Value::Null => None,
        Value::Number(n) => n.as_f64().filter(|f| !f.is_nan()),
        Value::String(s) => {
            let t = s.trim().replace(',', "");
            if t.is_empty() { None } else { t.parse().ok() }
        }
        _ => None,
    }
}

/// Cached OAuth token with its absolute expiry instant.
struct TokenCache {
    token: Option<String>,
    expiry: Instant,
}

/// Async-safe eBest API client.
pub struct EBestService {
    app_key: String,
    app_secret: String,
    base_url: String,
    http: reqwest::Client,
    token: Mutex<TokenCache>,
    next_call_at: Mutex<HashMap<String, Instant>>,
}

impl EBestService {
    /// Build a client from settings (TLS verification is configurable).
    pub fn new(settings: &Settings) -> Self {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(!settings.ebest_verify_ssl)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            app_key: settings.ebest_app_key.clone(),
            app_secret: settings.ebest_app_secret.clone(),
            base_url: settings.ebest_url.clone(),
            http,
            token: Mutex::new(TokenCache { token: None, expiry: Instant::now() }),
            next_call_at: Mutex::new(HashMap::new()),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Return a valid OAuth token, cached until 2 min before expiry.
    pub async fn auth_token(&self, force_refresh: bool) -> Option<String> {
        let mut cache = self.token.lock().await;
        if !force_refresh {
            if let Some(t) = &cache.token {
                if Instant::now() < cache.expiry {
                    return Some(t.clone());
                }
            }
        }
        let resp = self
            .http
            .post(format!("{}/oauth2/token", self.base_url))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "client_credentials"),
                ("appkey", self.app_key.as_str()),
                ("appsecretkey", self.app_secret.as_str()),
                ("scope", "oob"),
            ])
            .send()
            .await
            .ok()?;
        let data: Value = resp.json().await.ok()?;
        let token = data.get("access_token").and_then(Value::as_str)?.to_string();
        let expires_in = data.get("expires_in").and_then(Value::as_i64).unwrap_or(0);
        cache.token = Some(token.clone());
        // Refresh 2 minutes early to dodge the thundering-herd on expiry.
        cache.expiry = Instant::now() + Duration::from_secs((expires_in - 120).max(0) as u64);
        tracing::info!("[EBest] token refreshed (ttl={}s)", expires_in);
        Some(token)
    }

    fn is_token_error(result: &Value) -> bool {
        let code = result.get("rsp_cd").and_then(Value::as_str).unwrap_or("");
        let msg = result.get("rsp_msg").and_then(Value::as_str).unwrap_or("").to_lowercase();
        TOKEN_ERROR_CODES.contains(&code) || TOKEN_ERROR_KEYWORDS.iter().any(|kw| msg.contains(&kw.to_lowercase()))
    }

    fn is_rate_limited(result: &Value) -> bool {
        result.get("rsp_cd").and_then(Value::as_str) == Some(RATE_LIMIT_CODE)
    }

    /// Reserve the next slot for a TR code, then sleep — never blocks the runtime.
    async fn enforce_rate_limit(&self, tr_code: &str) {
        let wait = {
            let mut map = self.next_call_at.lock().await;
            let now = Instant::now();
            let slot = (*map.get(tr_code).unwrap_or(&now)).max(now);
            map.insert(tr_code.to_string(), slot + MIN_CALL_INTERVAL);
            slot.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            sleep(wait).await;
        }
    }

    /// POST a TR request (rate-limited) and return the parsed JSON body.
    async fn post_tr(&self, token: &str, path: &str, tr_code: &str, body: &Value) -> Value {
        self.enforce_rate_limit(tr_code).await;
        let resp = self
            .http
            .post(format!("{}/{}", self.base_url, path))
            .header("content-type", "application/json; charset=utf-8")
            .header("authorization", format!("Bearer {token}"))
            .header("tr_cd", tr_code)
            .header("tr_cont", "N")
            .header("tr_cont_key", "")
            .json(body)
            .send()
            .await;
        match resp {
            Ok(r) => r.json().await.unwrap_or_else(|_| json!({})),
            Err(e) => {
                tracing::error!("[EBest] {tr_code} error: {e}");
                json!({})
            }
        }
    }

    /// POST a TR with a one-shot token refresh + retry on token error.
    async fn post_tr_retry(&self, token: &str, path: &str, tr_code: &str, body: &Value) -> Value {
        let mut result = self.post_tr(token, path, tr_code, body).await;
        if Self::is_token_error(&result) {
            if let Some(fresh) = self.auth_token(true).await {
                result = self.post_tr(&fresh, path, tr_code, body).await;
            }
        }
        result
    }

    /// t1101 — current-price snapshot. Returns the OutBlock with `drate` added.
    pub async fn stock_price(&self, token: &str, shcode: &str) -> Value {
        let body = json!({"t1101InBlock": {"shcode": shcode}});
        let result = self.post_tr_retry(token, "stock/market-data", "t1101", &body).await;
        if let Some(block) = result.get("t1101OutBlock") {
            let mut block = block.clone();
            let drate = block.get("diff").and_then(parse_float).unwrap_or(0.0);
            block["drate"] = json!(drate);
            return block;
        }
        if Self::is_rate_limited(&result) {
            return json!({"_rate_limited": true});
        }
        json!({})
    }

    /// t8452 — intraday minute candles. Fills `edate` with today (KST) if blank.
    pub async fn fetch_minute_candles(&self, token: &str, shcode: &str, ncnt: i32, qrycnt: i32) -> Value {
        let edate = Utc::now().with_timezone(&Seoul).format("%Y%m%d").to_string();
        let body = json!({"t8452InBlock": {
            "shcode": shcode, "ncnt": ncnt, "qrycnt": qrycnt, "nday": "0",
            "sdate": "", "stime": "", "edate": edate, "etime": "",
            "cts_date": "", "cts_time": "", "comp_yn": "N", "exchgubun": "K",
        }});
        let mut result = self.post_tr_retry(token, "stock/chart", "t8452", &body).await;
        if Self::is_rate_limited(&result) {
            sleep(MIN_CALL_INTERVAL).await;
            result = self.post_tr(token, "stock/chart", "t8452", &body).await;
        }
        result
    }

    /// t8451 — daily candles (ascending OHLCV rows).
    pub async fn fetch_daily_candles(&self, token: &str, shcode: &str, qrycnt: i32) -> Vec<Value> {
        let now = Utc::now().with_timezone(&Seoul);
        let edate = now.format("%Y%m%d").to_string();
        let sdate = (now - chrono::Duration::days(qrycnt as i64 * 2)).format("%Y%m%d").to_string();
        let body = json!({"t8451InBlock": {
            "shcode": shcode, "gubun": "2", "qrycnt": qrycnt, "sdate": sdate, "edate": edate,
            "cts_date": "", "comp_yn": "N", "sujung": "Y", "exchgubun": "K",
        }});
        let mut result = self.post_tr_retry(token, "stock/chart", "t8451", &body).await;
        if Self::is_rate_limited(&result) {
            sleep(MIN_CALL_INTERVAL).await;
            result = self.post_tr(token, "stock/chart", "t8451", &body).await;
        }
        let mut rows: Vec<Value> = vec![];
        if let Some(items) = result.get("t8451OutBlock1").and_then(Value::as_array) {
            for it in items {
                let date = it.get("date").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let f = |k: &str| it.get(k).and_then(parse_float).unwrap_or(0.0);
                let (high, low) = (f("high"), f("low"));
                if !date.is_empty() && high > 0.0 && low > 0.0 {
                    rows.push(json!({
                        "date": date, "open": f("open"), "high": high, "low": low,
                        "close": f("close"), "volume": f("jdiff_vol"),
                    }));
                }
            }
        }
        rows.sort_by(|a, b| a["date"].as_str().cmp(&b["date"].as_str()));
        rows
    }

    /// CSPAT00601 — place a cash buy/sell order. `side`: "buy"|"sell".
    pub async fn place_order(&self, token: &str, code: &str, side: &str, qty: i64, price: f64, price_type: &str) -> Value {
        let isu_no = if code.starts_with('A') { code.to_string() } else { format!("A{code}") };
        let bns_tp = if side == "buy" { "2" } else { "1" };
        let body = json!({"CSPAT00601InBlock1": {
            "IsuNo": isu_no, "OrdQty": qty, "OrdPrc": price as i64,
            "BnsTpCode": bns_tp, "OrdprcPtnCode": price_type,
            "MgntrnCode": "000", "LoanDt": "", "OrdCndiTpCode": "0",
        }});
        self.post_tr_retry(token, "stock/order", "CSPAT00601", &body).await
    }

    /// t0424 — account balance (holdings + deposit).
    pub async fn get_account_balance(&self, token: &str) -> Value {
        let body = json!({"t0424InBlock": {
            "prcgb": "1", "chegb": "2", "dangb": "N", "charge": "N", "cts_expcode": "",
        }});
        self.post_tr_retry(token, "stock/accno", "t0424", &body).await
    }

    /// t0425 — 당일 주문 체결/미체결 조회. `ordno` 주문의
    /// `(체결수량, 평균체결가, 미체결잔량)` 을 돌려준다 (조회 실패 시 None).
    /// 지정가 주문의 체결 대사(fill reconciliation)에 사용한다.
    pub async fn order_fill_status(&self, token: &str, code: &str, ordno: i64) -> Option<(i64, f64, i64)> {
        let expcode = if code.starts_with('A') { code.to_string() } else { format!("A{code}") };
        let body = json!({"t0425InBlock": {
            "expcode": expcode, "chegb": "0", "medosu": "0", "sortgb": "1", "cts_ordno": " ",
        }});
        let res = self.post_tr_retry(token, "stock/accno", "t0425", &body).await;
        let rows = res.get("t0425OutBlock1").and_then(Value::as_array)?;
        for r in rows {
            let row_ordno = r.get("ordno").and_then(parse_float).unwrap_or(-1.0) as i64;
            if row_ordno != ordno {
                continue;
            }
            let cheqty = r.get("cheqty").and_then(parse_float).unwrap_or(0.0) as i64;
            let cheprice = r.get("cheprice").and_then(parse_float).unwrap_or(0.0);
            let ordrem = r.get("ordrem").and_then(parse_float).unwrap_or(0.0) as i64;
            return Some((cheqty, cheprice, ordrem));
        }
        None
    }

    /// CSPAT00801 — 취소주문 (지정가 미체결 잔량 취소). 성공 여부를 돌려준다.
    pub async fn cancel_order(&self, token: &str, ordno: i64, code: &str, qty: i64) -> bool {
        let isu_no = if code.starts_with('A') { code.to_string() } else { format!("A{code}") };
        let body = json!({"CSPAT00801InBlock1": {
            "OrgOrdNo": ordno, "IsuNo": isu_no, "OrdQty": qty,
        }});
        let res = self.post_tr_retry(token, "stock/order", "CSPAT00801", &body).await;
        let ok = res.get("rsp_cd").and_then(Value::as_str) == Some("0000");
        if !ok {
            tracing::warn!("[EBest] cancel_order {ordno} ({code}) failed: {:?}", res.get("rsp_msg"));
        }
        ok
    }
}
