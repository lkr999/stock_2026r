//! Shared application state (the singletons every handler reaches for).

use crate::candle_fetcher::CandleFetcher;
use crate::config::Settings;
use crate::ebest::EBestService;
use crate::engine::TradingEngine;
use crate::journal::TradeJournal;
use crate::pattern::PatternDetector;
use crate::telegram::TelegramNotifier;
use axum::http::StatusCode;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Process-wide singletons; cloned cheaply (everything is `Arc`) into handlers.
#[derive(Clone)]
pub struct AppState {
    pub settings: Arc<Settings>,
    pub ebest: Arc<EBestService>,
    pub fetcher: Arc<CandleFetcher>,
    pub detector: PatternDetector,
    pub journal: Arc<TradeJournal>,
    /// stock_monitor 방으로 상태/거래내역 요약을 보내는 텔레그램 알림 클라이언트.
    pub telegram: Arc<TelegramNotifier>,
    /// The trading engine is created on the first `/trading/start`.
    pub engine: Arc<Mutex<Option<Arc<TradingEngine>>>>,
}

impl AppState {
    /// Wire up all singletons at startup.
    pub fn new(settings: Settings) -> Self {
        let ebest = Arc::new(EBestService::new(&settings));
        let fetcher = Arc::new(CandleFetcher::new(ebest.clone()));
        Self {
            settings: Arc::new(settings),
            ebest,
            fetcher,
            detector: PatternDetector,
            journal: Arc::new(TradeJournal::new()),
            telegram: Arc::new(TelegramNotifier::from_env()),
            engine: Arc::new(Mutex::new(None)),
        }
    }

    /// Resolve an eBest auth token or fail with 503 — never substitute fake data.
    pub async fn token(&self) -> Result<String, (StatusCode, String)> {
        match self.ebest.auth_token(false).await {
            Some(t) if !t.is_empty() => Ok(t),
            // 게이트웨이가 알려준 실제 사유를 그대로 노출한다. 일반 문구만 띄우면
            // 앱키 만료인지 네트워크 장애인지 로그를 봐야만 알 수 있었다.
            _ => {
                let why = self
                    .ebest
                    .last_auth_error()
                    .await
                    .unwrap_or_else(|| "원인 불명 — 백엔드 로그를 확인하세요.".into());
                Err((StatusCode::SERVICE_UNAVAILABLE, format!("eBest 인증 실패 — {why}")))
            }
        }
    }
}
