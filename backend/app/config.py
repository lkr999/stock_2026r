"""Application settings loaded from environment / .env."""

from functools import lru_cache
from pathlib import Path

from pydantic_settings import BaseSettings, SettingsConfigDict

# config.py = backend/app/config.py → parents[2] = 프로젝트 루트
_PROJECT_ROOT = Path(__file__).resolve().parents[2]
# 루트 .env 와 backend/.env 를 모두 탐색 (실행 위치 무관하게 키 로드).
# 뒤쪽 항목이 우선하므로 backend/.env 가 루트 .env 를 덮어쓴다.
_ENV_FILES = (_PROJECT_ROOT / ".env", _PROJECT_ROOT / "backend" / ".env")


class Settings(BaseSettings):
    # eBest credentials
    EBEST_APP_KEY: str = ""
    EBEST_APP_SECRET: str = ""
    EBEST_URL: str = "https://openapi.ebestsec.co.kr"
    EBEST_VERIFY_SSL: bool = True

    # Server
    BACKEND_HOST: str = "0.0.0.0"
    BACKEND_PORT: int = 8000
    CORS_ORIGINS: str = "http://localhost:5173"

    # Trading (section 10)
    TRADING_MODE: str = "paper"  # paper | live
    TRADING_DEFAULT_STRATEGY: str = "balanced"
    TRADING_MAX_POSITIONS: int = 5
    TRADING_RISK_PER_TRADE: float = 0.01
    TRADING_DAILY_LOSS_LIMIT: float = 0.03
    TRADING_PAPER_SEED: float = 10_000_000

    # 주문 설정 (section 10-2)
    TRADING_ORDER_TYPE: str = "limit"        # limit=지정가 | market=시장가 | best=최유리지정가
    TRADING_FIXED_QTY: int = 0               # 1회 매수/매도 수량 (0=리스크 기반 자동 산정)
    TRADING_SELL_ALL: bool = True            # 매도 시 보유 전량 매도
    TRADING_MAX_BUY_AMOUNT: float = 500_000  # 1회 매수 한도액(원)

    # Live-trading readiness gate (section 11-2). Live is blocked until met.
    LIVE_MIN_PAPER_TRADES: int = 30
    LIVE_MIN_PAPER_DAYS: int = 14
    LIVE_MIN_WIN_RATE: float = 0.45
    LIVE_MIN_PROFIT_FACTOR: float = 1.3
    LIVE_REQUIRE_POSITIVE_PNL: bool = True
    LIVE_ALLOW_FORCE: bool = False  # if true, ?force=true can bypass the gate

    model_config = SettingsConfigDict(
        env_file=_ENV_FILES,
        env_file_encoding="utf-8",
        extra="ignore",
    )


@lru_cache
def get_settings() -> Settings:
    return Settings()
