//! Shared application state (the singletons every handler reaches for).

use crate::candle_fetcher::CandleFetcher;
use crate::config::Settings;
use crate::ebest::EBestService;
use crate::engine::TradingEngine;
use crate::journal::TradeJournal;
use crate::pattern::PatternDetector;
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
            engine: Arc::new(Mutex::new(None)),
        }
    }

    /// Resolve an eBest auth token or fail with 503 — never substitute fake data.
    pub async fn token(&self) -> Result<String, (StatusCode, String)> {
        match self.ebest.auth_token(false).await {
            Some(t) if !t.is_empty() => Ok(t),
            _ => Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "eBest 인증 실패 — 토큰을 발급할 수 없습니다. .env 의 \
                 EBEST_APP_KEY/EBEST_APP_SECRET 와 네트워크를 확인하세요."
                    .into(),
            )),
        }
    }
}
