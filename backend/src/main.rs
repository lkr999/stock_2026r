//! Caginalp candlestick trading system — Rust backend entrypoint.
//!
//! Wires the eBest client, pattern detector, journal, and trading engine into an
//! axum app exposing the same `/api/...` surface the SvelteKit frontend expects.
//! All market data comes from the eBest REST API only — never synthetic data.

mod backtest;
mod broker;
mod candle;
mod candle_fetcher;
mod config;
mod ebest;
mod engine;
mod indicators;
mod journal;
mod mtf;
mod pattern;
mod risk;
mod routers;
mod session;
mod state;
mod strategy;
mod telegram;
mod timeframe;
mod universe;
mod validation;

use axum::http::{HeaderValue, Method};
use config::Settings;
use state::AppState;
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    // Load `.env` from the project root and `backend/.env` (latter wins).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let _ = dotenvy::from_path(root.join(".env"));
    let _ = dotenvy::from_path(root.join("backend/.env"));
    tracing_subscriber::fmt().with_target(false).init();

    let settings = Settings::load();
    if !settings.ebest_configured() {
        tracing::warn!(
            "eBest 키 미설정 — .env 의 EBEST_APP_KEY/EBEST_APP_SECRET 를 설정하세요. \
             설정 전에는 데이터 조회 시 인증 오류가 반환됩니다 (가짜 데이터는 생성하지 않습니다)."
        );
    }
    let addr = format!("{}:{}", settings.backend_host, settings.backend_port);
    let cors = build_cors(&settings.cors_origins);
    let st = AppState::new(settings);

    // 자동 전송은 "보유 포지션 수 변동" 시에만. 나머지(주기/시작·정지 등)는
    // 수동(/chk 명령·웹 버튼)으로만 받는다.
    spawn_position_watcher(st.clone());
    // stock_monitor 방에서 /chk·/start·/stop 등 명령이 오면 즉시 응답한다.
    spawn_telegram_commands(st.clone());

    // Merge every resource router; all routes are absolute `/api/...` paths.
    let app = axum::Router::new()
        .merge(routers::misc::router())
        .merge(routers::candles::router())
        .merge(routers::patterns::router())
        .merge(routers::signals::router())
        .merge(routers::backtest::router())
        .merge(routers::trading::router())
        .layer(cors)
        .with_state(st);

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    tracing::info!("Caginalp Candlestick Trader listening on http://{addr}");
    axum::serve(listener, app).await.expect("serve");
}

/// 보유 포지션 수 변동 감시기. `TELEGRAM_POSITION_WATCH_SEC`(초, 기본 15) 간격으로
/// 엔진 상태를 확인해 **보유 포지션 개수가 바뀐 경우에만** 상태 리포트를
/// stock_monitor 방으로 자동 전송한다. 값이 0이면 자동 전송을 완전히 끈다(전부 수동).
/// 텔레그램 미구성이면 태스크를 띄우지 않는다.
fn spawn_position_watcher(st: AppState) {
    let interval: u64 = std::env::var("TELEGRAM_POSITION_WATCH_SEC").ok().and_then(|v| v.parse().ok()).unwrap_or(15);
    if interval == 0 || !st.telegram.configured() {
        return;
    }
    tracing::info!("[telegram] 포지션 변동 자동알림 활성화 — {interval}초 간격 확인");
    tokio::spawn(async move {
        // 최초 관측값을 기준선으로 잡고(즉시 전송하지 않음), 이후 변동 시에만 전송.
        // 재시작 시 복원된 포지션 수로 오알림이 뜨지 않도록 None 으로 시작한다.
        let mut last: Option<usize> = None;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval));
        loop {
            tick.tick().await;
            let engine = st.engine.lock().await.clone();
            let count = match &engine {
                Some(e) => e
                    .status()
                    .await
                    .get("positions")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| a.len())
                    .unwrap_or(0),
                None => 0,
            };
            match last {
                None => last = Some(count),
                Some(prev) if prev != count => {
                    let body = telegram::build_status_text(engine, &st.journal).await;
                    let msg = format!("🔔 보유 포지션 변동 ({prev} → {count})\n\n{body}");
                    if let Err(e) = st.telegram.send(&msg).await {
                        tracing::warn!("[telegram] 포지션 변동 알림 전송 실패: {e}");
                    }
                    last = Some(count);
                }
                _ => {}
            }
        }
    });
}

/// 텔레그램 명령 리스너. getUpdates 롱폴링으로 stock_monitor 방의 메시지를
/// 감시하다가 `/chk` 명령을 받으면 현재 상태 리포트를 그 방으로 응답한다.
/// 보안상 설정된 chat_id(stock_monitor)에서 온 명령에만 반응한다. 텔레그램
/// 미구성이면 태스크를 띄우지 않는다.
fn spawn_telegram_commands(st: AppState) {
    if !st.telegram.configured() {
        return;
    }
    tracing::info!("[telegram] /chk 명령 리스너 활성화");
    tokio::spawn(async move {
        // 시작 시점의 이전(백로그) 명령은 무시하고 최신 offset 만 확보한다 —
        // 재시작 때 예전 /chk 가 재실행되는 것을 막는다.
        let mut offset: i64 = match st.telegram.get_updates(0, 0).await {
            Ok((next, _)) => next,
            Err(e) => {
                tracing::warn!("[telegram] 초기 offset 조회 실패: {e}");
                0
            }
        };
        loop {
            match st.telegram.get_updates(offset, 30).await {
                Ok((next, msgs)) => {
                    offset = next;
                    for (chat_id, text) in msgs {
                        // 인가된 방(stock_monitor)에서 온 명령만 처리.
                        if chat_id != st.telegram.chat_id() {
                            continue;
                        }
                        let Some(cmd) = telegram::parse_command(&text) else { continue };
                        let reply = handle_telegram_command(&st, cmd).await;
                        if let Err(e) = st.telegram.send_to(&chat_id, &reply).await {
                            tracing::warn!("[telegram] 명령 응답 전송 실패: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[telegram] getUpdates 실패(5초 후 재시도): {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}

/// 원격 텔레그램 명령을 실행하고 응답 텍스트를 만든다.
async fn handle_telegram_command(st: &AppState, cmd: telegram::Command) -> String {
    use telegram::Command;
    match cmd {
        Command::Help => telegram::help_text(),
        Command::Chk(kind) => {
            let engine = st.engine.lock().await.clone();
            telegram::build_chk(engine, &st.journal, kind).await
        }
        Command::Start => {
            let engine = st.engine.lock().await.clone();
            match engine {
                None => "⚠️ 설정된 엔진이 없습니다. 웹 화면에서 관심종목·전략을 한 번 설정해 시작한 뒤에는 /start 로 재개할 수 있습니다.".into(),
                Some(e) => {
                    if e.running().await {
                        "ℹ️ 이미 자동매매가 실행 중입니다.".into()
                    } else {
                        // 기존 설정/포지션/현금을 유지한 채 재개(resume=true).
                        let poll = e.status().await.get("poll_sec").and_then(serde_json::Value::as_u64).unwrap_or(60);
                        e.start(poll, true).await;
                        format!("▶️ 자동매매 시작\n\n{}", telegram::status_section(&e.status().await))
                    }
                }
            }
        }
        Command::Stop => {
            let engine = st.engine.lock().await.clone();
            match engine {
                None => "ℹ️ 실행 중인 엔진이 없습니다.".into(),
                Some(e) => {
                    e.stop().await;
                    format!("⏹ 자동매매 중지\n\n{}", telegram::status_section(&e.status().await))
                }
            }
        }
    }
}

/// CORS allowing the configured origins plus the SvelteKit dev server.
fn build_cors(origins: &str) -> CorsLayer {
    let mut list: Vec<HeaderValue> = origins
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    if let Ok(dev) = "http://localhost:7777".parse() {
        if !list.contains(&dev) {
            list.push(dev);
        }
    }
    CorsLayer::new()
        .allow_origin(list)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers(tower_http::cors::Any)
}
