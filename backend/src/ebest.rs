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
/// 전송 실패(응답을 못 받음) 시 추가 재시도 횟수 — 조회 TR 에만 적용한다.
const TRANSPORT_RETRIES: usize = 2;

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
    /// 마지막 발급 실패 사유 + 시각. 사유는 503 응답에 실어 보내고, 시각은
    /// 자격증명이 죽었을 때 게이트웨이를 두들기지 않도록 쿨다운에 쓴다.
    last_error: Option<String>,
    failed_at: Option<Instant>,
}

/// 토큰 발급이 실패한 뒤 재시도까지의 최소 간격. 앱키가 만료/폐기된 상태에서는
/// 모든 API 요청이 각자 /oauth2/token 을 때리게 되는데, 그대로 두면 초당 수십 건의
/// 403 이 나가 게이트웨이 차단을 부른다.
const AUTH_RETRY_COOLDOWN: Duration = Duration::from_secs(10);

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
            .connect_timeout(Duration::from_secs(10))
            // LS 게이트웨이는 유휴 keep-alive 커넥션을 예고 없이 끊는다. reqwest 기본
            // 유휴 보관시간(90초)은 그보다 길어서, 이미 죽은 커넥션을 풀에서 꺼내
            // 재사용하다 "error sending request for url ..." 로 실패하는 일이 생긴다.
            // 짧게 잡아 살아있는 커넥션만 재사용하도록 한다.
            .pool_idle_timeout(Duration::from_secs(15))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            app_key: settings.ebest_app_key.clone(),
            app_secret: settings.ebest_app_secret.clone(),
            base_url: settings.ebest_url.clone(),
            http,
            token: Mutex::new(TokenCache {
                token: None,
                expiry: Instant::now(),
                last_error: None,
                failed_at: None,
            }),
            next_call_at: Mutex::new(HashMap::new()),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Return a valid OAuth token, cached until 2 min before expiry.
    ///
    /// 실패하면 사유를 `last_auth_error()` 에 남긴다 — 예전에는 `.ok()?` 로 전부
    /// 삼켜서 "앱키 만료(403)"와 "DNS 실패"가 똑같이 빈 결과로 보였다.
    pub async fn auth_token(&self, force_refresh: bool) -> Option<String> {
        let mut cache = self.token.lock().await;
        if !force_refresh {
            if let Some(t) = &cache.token {
                if Instant::now() < cache.expiry {
                    return Some(t.clone());
                }
            }
        }
        // 직전 실패 직후면 재요청하지 않고 같은 사유를 그대로 돌려준다.
        if let Some(at) = cache.failed_at {
            if at.elapsed() < AUTH_RETRY_COOLDOWN {
                return None;
            }
        }
        if self.app_key.is_empty() || self.app_secret.is_empty() {
            Self::record_failure(
                &mut cache,
                ".env 의 EBEST_APP_KEY/EBEST_APP_SECRET 가 비어 있습니다.".into(),
            );
            return None;
        }

        let sent = self
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
            .await;
        let resp = match sent {
            Ok(r) => r,
            Err(e) => {
                let why = format!("{} 접속 실패 ({})", self.base_url, Self::describe(&e));
                Self::record_failure(&mut cache, why);
                return None;
            }
        };
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let data: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

        let Some(token) = data.get("access_token").and_then(Value::as_str) else {
            Self::record_failure(&mut cache, Self::auth_failure_reason(status, &data, &body));
            return None;
        };
        let token = token.to_string();
        let expires_in = data.get("expires_in").and_then(Value::as_i64).unwrap_or(0);
        cache.token = Some(token.clone());
        // Refresh 2 minutes early to dodge the thundering-herd on expiry.
        cache.expiry = Instant::now() + Duration::from_secs((expires_in - 120).max(0) as u64);
        cache.last_error = None;
        cache.failed_at = None;
        tracing::info!("[EBest] token refreshed (ttl={}s)", expires_in);
        Some(token)
    }

    /// 게이트웨이가 돌려준 error_code 를 사람이 바로 행동할 수 있는 문장으로 옮긴다.
    fn auth_failure_reason(status: reqwest::StatusCode, data: &Value, body: &str) -> String {
        let code = data.get("error_code").and_then(Value::as_str).unwrap_or("");
        let desc = data
            .get("error_description")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| body.trim().chars().take(200).collect());
        let hint = match code {
            // 서버가 앱키 자체를 모른다 — 오타가 아니라 만료/폐기인 경우가 대부분이다.
            "IGW00103" => " 앱키가 LS증권 서버에 등록돼 있지 않습니다(유효기간 만료 또는 폐기). \
                            LS증권 OpenAPI 에서 앱키를 재발급한 뒤 backend/.env 의 \
                            EBEST_APP_KEY/EBEST_APP_SECRET 를 갱신하고 백엔드를 재시작하세요.",
            "IGW00121" | "IGW00133" => " AppSecretKey 가 앱키와 짝이 맞지 않습니다. \
                                        재발급 시 받은 쌍을 그대로 넣었는지 확인하세요.",
            _ => "",
        };
        format!("HTTP {} {} {}{}", status.as_u16(), code, desc, hint)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn record_failure(cache: &mut TokenCache, why: String) {
        tracing::error!("[EBest] 토큰 발급 실패 — {why}");
        cache.token = None;
        cache.last_error = Some(why);
        cache.failed_at = Some(Instant::now());
    }

    /// 마지막 토큰 발급 실패 사유 (없으면 None).
    pub async fn last_auth_error(&self) -> Option<String> {
        self.token.lock().await.last_error.clone()
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

    /// 주문 계열 TR — 전송이 실패해도 요청이 서버에 도달했을 수 있으므로
    /// 자동 재시도하지 않는다 (중복 주문 위험).
    fn is_order_tr(tr_code: &str) -> bool {
        tr_code.starts_with("CSPAT")
    }

    /// reqwest 0.12 의 Display 는 최상위 메시지("error sending request for url ...")만
    /// 남기고 실제 원인(타임아웃 / 커넥션 끊김 / DNS)은 source 체인에 숨긴다.
    /// 원인까지 펼쳐서 로그 한 줄만으로 구분할 수 있게 한다.
    fn describe(e: &reqwest::Error) -> String {
        let kind = if e.is_timeout() {
            "timeout"
        } else if e.is_connect() {
            "connect"
        } else if e.is_body() {
            "body"
        } else if e.is_decode() {
            "decode"
        } else {
            "request"
        };
        let mut chain: Vec<String> = vec![];
        let mut src = std::error::Error::source(e);
        while let Some(s) = src {
            chain.push(s.to_string());
            src = s.source();
        }
        if chain.is_empty() { kind.to_string() } else { format!("{kind}: {}", chain.join(" <- ")) }
    }

    /// POST a TR request (rate-limited) and return the parsed JSON body.
    /// 전송 단계 실패는 조회 TR 에 한해 재시도한다 — 유휴 커넥션이 끊겼거나
    /// 일시적인 네트워크 단절이면 새 커넥션으로 바로 복구된다.
    async fn post_tr(&self, token: &str, path: &str, tr_code: &str, body: &Value) -> Value {
        let attempts = if Self::is_order_tr(tr_code) { 1 } else { 1 + TRANSPORT_RETRIES };
        for attempt in 1..=attempts {
            // 재시도도 레이트리밋 슬롯을 새로 잡는다 (~1.1초 간격 = 백오프 역할).
            self.enforce_rate_limit(tr_code).await;
            let sent = self
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
            // 본문 수신 중 끊김(is_body)도 같은 전송 실패로 취급해 함께 재시도한다.
            let err = match sent {
                Ok(r) => match r.json::<Value>().await {
                    Ok(v) => return v,
                    Err(e) => e,
                },
                Err(e) => e,
            };
            let detail = Self::describe(&err);
            if attempt < attempts {
                tracing::warn!("[EBest] {tr_code} transport error ({detail}) — retry {attempt}/{}", attempts - 1);
            } else {
                tracing::error!("[EBest] {tr_code} failed after {attempt} attempt(s): {detail}");
            }
        }
        json!({})
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
    /// `cts` = (cts_date, cts_time) 연속조회 키 — 빈 문자열이면 최신 구간.
    pub async fn fetch_minute_candles_cts(&self, token: &str, shcode: &str, ncnt: i32, qrycnt: i32, cts: (&str, &str)) -> Value {
        let edate = Utc::now().with_timezone(&Seoul).format("%Y%m%d").to_string();
        let body = json!({"t8452InBlock": {
            "shcode": shcode, "ncnt": ncnt, "qrycnt": qrycnt, "nday": "0",
            "sdate": "", "stime": "", "edate": edate, "etime": "",
            "cts_date": cts.0, "cts_time": cts.1, "comp_yn": "N", "exchgubun": "K",
        }});
        let mut result = self.post_tr_retry(token, "stock/chart", "t8452", &body).await;
        if Self::is_rate_limited(&result) {
            sleep(MIN_CALL_INTERVAL).await;
            result = self.post_tr(token, "stock/chart", "t8452", &body).await;
        }
        result
    }

    pub async fn fetch_minute_candles(&self, token: &str, shcode: &str, ncnt: i32, qrycnt: i32) -> Value {
        self.fetch_minute_candles_cts(token, shcode, ncnt, qrycnt, ("", "")).await
    }

    /// t8452 응답에서 다음 페이지의 연속조회 키를 꺼낸다 (없으면 None = 마지막 페이지).
    pub fn minute_cts_key(result: &Value) -> Option<(String, String)> {
        let b = result.get("t8452OutBlock")?;
        let d = b.get("cts_date").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let t = b.get("cts_time").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if d.is_empty() { None } else { Some((d, t)) }
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
