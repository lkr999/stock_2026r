//! 텔레그램 알림 — 봇을 통해 트레이딩 "상태 / 모니터링 현황 / 최근 거래내역"을
//! 하나의 메시지로 정리해 stock_monitor 방으로 전송한다.
//!
//! 설정은 `.env` 에서 읽는다:
//!   TELEGRAM_BOT_TOKEN=<BotFather 로 발급한 토큰>
//!   TELEGRAM_CHAT_ID=<stock_monitor 방의 chat_id (그룹은 -100... 형태)>
//!   TELEGRAM_BASE_URL=https://api.telegram.org/bot   (기본값)
//!   TELEGRAM_REPORT_INTERVAL_SEC=0                   (0 = 주기 전송 끔)

use crate::engine::TradingEngine;
use crate::journal::TradeJournal;
use chrono::DateTime;
use chrono_tz::Asia::Seoul;
use serde_json::{json, Value};
use std::sync::Arc;

/// 텔레그램 봇 클라이언트 (env 에서 구성).
#[derive(Clone)]
pub struct TelegramNotifier {
    client: reqwest::Client,
    token: String,
    chat_id: String,
    base_url: String,
}

impl TelegramNotifier {
    /// `.env` 로부터 봇 토큰/채팅 ID/베이스 URL 을 읽어 구성한다.
    /// 값에 붙은 인라인 주석(`1470780464 #개인 대화방`)은 방어적으로 제거한다.
    pub fn from_env() -> Self {
        Self {
            // 롱폴링(getUpdates, timeout=30s)보다 넉넉한 요청 타임아웃.
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(45))
                .build()
                .unwrap_or_default(),
            // stock_monitor 전용 키를 우선 참조하고, 없으면 기존 TELEGRAM_* 로 폴백.
            token: clean(&env_any(&["STOCK_BOT_TOKEN", "TELEGRAM_BOT_TOKEN"])),
            chat_id: clean(&env_any(&["STOCK_CHAT_ID", "TELEGRAM_CHAT_ID"])),
            base_url: {
                let b = env("TELEGRAM_BASE_URL");
                if b.trim().is_empty() { "https://api.telegram.org/bot".into() } else { b.trim().to_string() }
            },
        }
    }

    /// 토큰과 채팅 ID 가 모두 있어야 전송 가능하다.
    pub fn configured(&self) -> bool {
        !self.token.is_empty() && !self.chat_id.is_empty()
    }

    /// 설정된 stock_monitor 방의 chat_id (명령 발신자 검증용).
    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    /// 임의의 텍스트를 stock_monitor 방으로 전송한다. 텔레그램 4096자 제한을
    /// 넘으면 줄 단위로 잘라 여러 메시지로 나눠 보낸다.
    pub async fn send(&self, text: &str) -> Result<(), String> {
        if !self.configured() {
            return Err("텔레그램 미설정 — .env 의 TELEGRAM_BOT_TOKEN / TELEGRAM_CHAT_ID 를 확인하세요.".into());
        }
        self.send_to(&self.chat_id, text).await
    }

    /// 지정한 chat_id 로 텍스트를 전송한다(4096자 초과 시 분할).
    pub async fn send_to(&self, chat_id: &str, text: &str) -> Result<(), String> {
        if self.token.is_empty() || chat_id.is_empty() {
            return Err("텔레그램 미설정 — TELEGRAM_BOT_TOKEN / chat_id 를 확인하세요.".into());
        }
        let url = format!("{}{}/sendMessage", self.base_url, self.token);
        for chunk in split_chunks(text, 3900) {
            let body = json!({
                "chat_id": chat_id,
                "text": chunk,
                "disable_web_page_preview": true,
            });
            let resp = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("텔레그램 요청 실패: {e}"))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let msg = resp.text().await.unwrap_or_default();
                return Err(format!("텔레그램 전송 실패 ({status}): {msg}"));
            }
        }
        Ok(())
    }

    /// getUpdates 롱폴링. `offset` 이후의 업데이트를 최대 `timeout_sec` 초 대기하며
    /// 받아 `(다음 offset, [(chat_id, 텍스트)])` 를 돌려준다.
    pub async fn get_updates(&self, offset: i64, timeout_sec: u64) -> Result<(i64, Vec<(String, String)>), String> {
        let url = format!("{}{}/getUpdates", self.base_url, self.token);
        let resp = self
            .client
            .get(&url)
            .query(&[("offset", offset.to_string()), ("timeout", timeout_sec.to_string())])
            .send()
            .await
            .map_err(|e| format!("getUpdates 요청 실패: {e}"))?;
        let v: Value = resp.json().await.map_err(|e| format!("getUpdates 파싱 실패: {e}"))?;
        let mut next = offset;
        let mut msgs = Vec::new();
        if let Some(results) = v.get("result").and_then(Value::as_array) {
            for u in results {
                let uid = u.get("update_id").and_then(Value::as_i64).unwrap_or(0);
                if uid >= next {
                    next = uid + 1;
                }
                if let Some(m) = u.get("message") {
                    let chat_id = m
                        .get("chat")
                        .and_then(|c| c.get("id"))
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    let text = m.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                    if !chat_id.is_empty() && !text.is_empty() {
                        msgs.push((chat_id, text));
                    }
                }
            }
        }
        Ok((next, msgs))
    }
}

/// 봇이 처리하는 원격 명령.
#[derive(Clone, Copy, PartialEq)]
pub enum Command {
    /// `/chk`(전체) · `/chk1`(상태) · `/chk2`(모니터링) · `/chk3`(거래내역)
    Chk(ChkKind),
    /// `/start` — 자동매매 시작(기존 엔진 재개)
    Start,
    /// `/stop` — 자동매매 중지
    Stop,
    /// `/help` — 명령 안내
    Help,
}

/// `/chk` 계열이 보여줄 섹션 구분.
#[derive(Clone, Copy, PartialEq)]
pub enum ChkKind {
    All,
    Status,
    Monitor,
    Trades,
}

/// 텔레그램 메시지 첫 토큰을 명령으로 파싱한다(`/chk@봇이름`의 `@…`는 무시).
pub fn parse_command(text: &str) -> Option<Command> {
    let first = text.trim().split_whitespace().next().unwrap_or("");
    let cmd = first.split('@').next().unwrap_or("").to_ascii_lowercase();
    match cmd.as_str() {
        "/chk" => Some(Command::Chk(ChkKind::All)),
        "/chk1" => Some(Command::Chk(ChkKind::Status)),
        "/chk2" => Some(Command::Chk(ChkKind::Monitor)),
        "/chk3" => Some(Command::Chk(ChkKind::Trades)),
        "/start" => Some(Command::Start),
        "/stop" => Some(Command::Stop),
        "/help" => Some(Command::Help),
        _ => None,
    }
}

/// 명령 안내 문구.
pub fn help_text() -> String {
    "🤖 stock_monitor 명령\n──────────────\n\
     /start — 자동매매 시작\n\
     /stop — 자동매매 중지\n\
     /chk — 전체(상태+모니터링+거래내역)\n\
     /chk1 — 상태 요약\n\
     /chk2 — 모니터링 현황\n\
     /chk3 — 최근 거래내역\n\
     /help — 이 안내"
        .to_string()
}

/// 엔진 상태 + 저널을 취합해 `/chk` 계열 리포트 본문을 만든다(전송은 하지 않음).
/// 엔진이 아직 없으면(미시작) 정지 상태 스텁으로 보고한다.
pub async fn build_chk(engine: Option<Arc<TradingEngine>>, journal: &TradeJournal, kind: ChkKind) -> String {
    let status = match &engine {
        Some(e) => e.status().await,
        None => json!({
            "running": false, "mode": "paper", "cycles": 0,
            "cash": 0.0, "equity": 0.0, "daily_pnl": 0.0,
            "positions": [], "monitor": [], "monitor_summary": {},
        }),
    };
    // 거래내역이 필요한 섹션에서만 저널을 읽는다.
    let recent = if matches!(kind, ChkKind::All | ChkKind::Trades) {
        let mode = status.get("mode").and_then(Value::as_str).unwrap_or("paper");
        let mut trades = journal.by_mode(mode);
        let start = trades.len().saturating_sub(40);
        let mut r: Vec<Value> = trades.drain(start..).collect();
        r.reverse(); // 최신 거래가 위로
        r
    } else {
        Vec::new()
    };
    match kind {
        ChkKind::All => {
            format!("{}\n{}\n{}", status_section(&status), monitor_section(&status), trades_section(&recent))
        }
        ChkKind::Status => status_section(&status),
        ChkKind::Monitor => monitor_section(&status),
        ChkKind::Trades => trades_section(&recent),
    }
}

/// 전체 리포트(상태+모니터링+거래내역). 주기/시작·정지 알림에서 사용.
pub async fn build_status_text(engine: Option<Arc<TradingEngine>>, journal: &TradeJournal) -> String {
    build_chk(engine, journal, ChkKind::All).await
}

/// 엔진 상태 + 저널을 취합해 "상태/모니터링/최근 거래내역" 리포트를 전송한다.
/// 엔진이 아직 없으면(미시작) 정지 상태 스텁으로 보고한다.
pub async fn send_status_report(
    notifier: &TelegramNotifier,
    engine: Option<Arc<TradingEngine>>,
    journal: &TradeJournal,
) -> Result<(), String> {
    let text = build_status_text(engine, journal).await;
    notifier.send(&text).await
}

/// ① 상태 섹션 — `/chk1`. 상태/모드/사이클/현금/평가자산/미실현/일손익.
pub fn status_section(status: &Value) -> String {
    let running = status.get("running").and_then(Value::as_bool).unwrap_or(false);
    let mode = status.get("mode").and_then(Value::as_str).unwrap_or("paper");
    let cycles = status.get("cycles").and_then(Value::as_i64).unwrap_or(0);
    let cash = status.get("cash").and_then(Value::as_f64).unwrap_or(0.0);
    let equity = status.get("equity").and_then(Value::as_f64).unwrap_or(0.0);
    let daily_pnl = status.get("daily_pnl").and_then(Value::as_f64).unwrap_or(0.0);
    let unrealized: f64 = status
        .get("positions")
        .and_then(Value::as_array)
        .map(|ps| ps.iter().map(|p| p.get("unrealized_pnl").and_then(Value::as_f64).unwrap_or(0.0)).sum())
        .unwrap_or(0.0);

    let mode_ko = if mode == "live" { "실전투자" } else { "모의투자" };
    let state_ko = if running { "🟢 실행중" } else { "🔴 정지" };

    let mut out = String::new();
    out.push_str(&format!("📊 트레이딩 상태  ({})\n", now_kst()));
    out.push_str("──────────────\n");
    out.push_str(&format!("상태: {state_ko}\n"));
    out.push_str(&format!("모드: {mode_ko}\n"));
    out.push_str(&format!("사이클: {cycles}\n"));
    out.push_str(&format!("현금: {}\n", won(cash)));
    out.push_str(&format!("평가자산: {}\n", won(equity)));
    out.push_str(&format!("미실현: {}\n", won_signed(unrealized)));
    out.push_str(&format!("일손익: {}\n", won_signed(daily_pnl)));
    out
}

/// ② 모니터링 섹션 — `/chk2`. 보유 포지션 + 감시 단계 요약.
pub fn monitor_section(status: &Value) -> String {
    let positions = status.get("positions").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = String::new();
    out.push_str("🖥 모니터링 현황\n──────────────\n");
    if positions.is_empty() {
        out.push_str("보유 포지션: 없음\n");
    } else {
        out.push_str(&format!("보유 포지션: {}개\n", positions.len()));
        for p in &positions {
            let name = p.get("name").and_then(Value::as_str).unwrap_or("");
            let code = p.get("code").and_then(Value::as_str).unwrap_or("");
            let qty = p.get("qty").and_then(Value::as_i64).unwrap_or(0);
            let cur = p.get("current_price").and_then(Value::as_f64).unwrap_or(0.0);
            let upnl = p.get("unrealized_pnl").and_then(Value::as_f64).unwrap_or(0.0);
            let upct = p.get("unrealized_pct").and_then(Value::as_f64).unwrap_or(0.0);
            out.push_str(&format!(
                "• {name}({code}) {qty}주 @{} {} ({:+.2}%)\n",
                won(cur),
                won_signed(upnl),
                upct,
            ));
        }
    }
    if let Some(summary) = status.get("monitor_summary").and_then(Value::as_object) {
        if !summary.is_empty() {
            let mut parts: Vec<String> = summary
                .iter()
                .map(|(phase, n)| format!("{phase} {}", n.as_i64().unwrap_or(0)))
                .collect();
            parts.sort();
            out.push_str(&format!("감시 단계: {}\n", parts.join(" · ")));
        }
    }
    out
}

/// ③ 거래내역 섹션 — `/chk3`. 최근 거래(최대 40건).
pub fn trades_section(trades: &[Value]) -> String {
    let mut out = String::new();
    out.push_str(&format!("📒 거래 내역 (최근 {}건)\n──────────────\n", trades.len().min(40)));
    if trades.is_empty() {
        out.push_str("거래 없음\n");
    } else {
        for t in trades {
            let code = t.get("code").and_then(Value::as_str).unwrap_or("");
            let pnl = t.get("pnl").and_then(Value::as_f64).unwrap_or(0.0);
            let ret = t.get("return_pct").and_then(Value::as_f64).unwrap_or(0.0);
            let reason = t.get("reason").and_then(Value::as_str).unwrap_or("");
            let closed = t.get("closed_at").and_then(Value::as_str).unwrap_or("");
            let mark = if pnl > 0.0 { "🔺" } else if pnl < 0.0 { "🔻" } else { "▪️" };
            out.push_str(&format!(
                "{mark} {} {code} {} ({:+.2}%) {}\n",
                hhmm_kst(closed),
                won_signed(pnl),
                ret,
                reason,
            ));
        }
    }
    out
}

// ── 포매팅 헬퍼 ────────────────────────────────────────────────

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

/// 주어진 키들을 순서대로 조회해 처음으로 비어있지 않은 값을 반환한다.
fn env_any(keys: &[&str]) -> String {
    for k in keys {
        let v = env(k);
        if !clean(&v).is_empty() {
            return v;
        }
    }
    String::new()
}

/// 인라인 주석/공백 제거: `"1470780464 #개인 대화방"` → `"1470780464"`.
fn clean(raw: &str) -> String {
    raw.split('#').next().unwrap_or("").split_whitespace().next().unwrap_or("").to_string()
}

/// 천단위 구분 원화 표기: `1234567.0` → `"1,234,567원"`.
fn won(v: f64) -> String {
    format!("{}원", group(v.round() as i64))
}

/// 부호 포함 원화 표기: `-1200.0` → `"-1,200원"`, `0.0` → `"+0원"`.
fn won_signed(v: f64) -> String {
    let n = v.round() as i64;
    let sign = if n < 0 { "-" } else { "+" };
    format!("{sign}{}원", group(n.abs()))
}

/// 정수 천단위 콤마.
fn group(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if neg { format!("-{out}") } else { out }
}

/// RFC3339(UTC) → KST "MM-DD HH:MM". 파싱 실패 시 원문 앞 16자.
fn hhmm_kst(iso: &str) -> String {
    match DateTime::parse_from_rfc3339(iso) {
        Ok(dt) => dt.with_timezone(&Seoul).format("%m-%d %H:%M").to_string(),
        Err(_) => iso.chars().take(16).collect(),
    }
}

/// 현재 KST 시각 "YYYY-MM-DD HH:MM".
fn now_kst() -> String {
    chrono::Utc::now().with_timezone(&Seoul).format("%Y-%m-%d %H:%M").to_string()
}

/// 텔레그램 길이 제한 대응: 줄 경계에서 `max` 바이트 이하로 분할.
fn split_chunks(text: &str, max: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if cur.len() + line.len() + 1 > max && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}
