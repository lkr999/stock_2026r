"""Injectable eBest broker REST API client for LLM Trader."""

import asyncio
import logging
import ssl
import time
from datetime import datetime, timedelta, timezone
from typing import Any

import aiohttp

from app.config import get_settings

logger = logging.getLogger(__name__)

KST = timezone(timedelta(hours=9))

MIN_CALL_INTERVAL: float = 1.1  # seconds — eBest 호출 간격(콜 시작 기준 약 1콜/초)
# 실측 결과 콜 '시작' 간격이 1초 미만이면 IGW00201(호출건수 초과)이 발생한다.
# 즉 차트/시세 TR은 ~1콜/초가 한도이므로 간격을 더 줄일 수 없다. 대량 종목
# 스캔 속도는 CandleFetcher 캐시와 스캔 종목 수로 조절한다(코드 호출당 한도는 고정).
_TOKEN_ERROR_CODES = {"IGW00123", "IGW00124", "IGW00125"}
_TOKEN_ERROR_KEYWORDS = (
    "유효하지 않은 token",
    "token이 없습니다",
    "token 만료",
    "invalid token",
    "unauthorized",
)
_RATE_LIMIT_ERROR_CODES = {"IGW00201"}


def _create_ssl_context(verify: bool) -> ssl.SSLContext:
    ctx = ssl.create_default_context()
    if not verify:
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
    return ctx


def _parse_ebest_float(value: Any) -> float | None:
    if value is None:
        return None
    if isinstance(value, (int, float)):
        if value != value:  # NaN
            return None
        return float(value)
    s = str(value).strip().replace(",", "")
    if not s:
        return None
    try:
        return float(s)
    except ValueError:
        return None


def _t1101_change_rate_percent(block: dict[str, Any]) -> float:
    """등락률(%): t1101OutBlock.diff 를 등락율 수치로 그대로 사용 (추가 환산 없음)."""
    if not block:
        return 0.0
    v = _parse_ebest_float(block.get("diff"))
    return float(v) if v is not None else 0.0


class EBestService:
    """Service-layer wrapper around the eBest broker REST API."""

    def __init__(
        self,
        app_key: str | None = None,
        app_secret: str | None = None,
        base_url: str | None = None,
    ) -> None:
        settings = get_settings()
        self.app_key = app_key if app_key is not None else settings.EBEST_APP_KEY
        self.app_secret = app_secret if app_secret is not None else settings.EBEST_APP_SECRET
        self.base_url = base_url if base_url is not None else settings.EBEST_URL

        self._ssl_context = _create_ssl_context(settings.EBEST_VERIFY_SSL)
        self._session: aiohttp.ClientSession | None = None
        self._session_loop: asyncio.AbstractEventLoop | None = None

        self._cached_token: str | None = None
        self._token_expiry: float = 0.0
        self._token_lock = asyncio.Lock()

        self._rate_lock = asyncio.Lock()
        self._next_call_at: dict[str, float] = {}

    # ------------------------------------------------------------------
    # lifecycle
    # ------------------------------------------------------------------

    async def start(self) -> None:
        await self._ensure_session()
        logger.info("EBestService started")

    async def aclose(self) -> None:
        if self._session and not self._session.closed:
            try:
                if self._session_loop == asyncio.get_running_loop():
                    await asyncio.wait_for(self._session.close(), timeout=2.0)
            except Exception:
                pass  # ignore SSL shutdown timeouts
        self._session = None
        self._session_loop = None
        logger.info("EBestService closed")

    async def _ensure_session(self) -> aiohttp.ClientSession:
        current_loop = asyncio.get_running_loop()

        if self._session is None or self._session.closed:
            connector = aiohttp.TCPConnector(ssl=self._ssl_context, limit=100)
            timeout = aiohttp.ClientTimeout(total=30)
            self._session = aiohttp.ClientSession(connector=connector, timeout=timeout)
            self._session_loop = current_loop
            return self._session

        if self._session_loop != current_loop:
            try:
                if self._session_loop and self._session_loop.is_running():
                    fut = asyncio.run_coroutine_threadsafe(
                        self._session.close(), self._session_loop
                    )
                    fut.result(timeout=2.0)
            except Exception:
                pass

            connector = aiohttp.TCPConnector(ssl=self._ssl_context, limit=100)
            timeout = aiohttp.ClientTimeout(total=30)
            self._session = aiohttp.ClientSession(connector=connector, timeout=timeout)
            self._session_loop = current_loop

        return self._session

    # ------------------------------------------------------------------
    # auth + rate limiting
    # ------------------------------------------------------------------

    async def auth_token(self, force_refresh: bool = False) -> str | None:
        """Return a valid OAuth token. Cached until 2 min before expiry.

        Token refresh is serialized through `_token_lock` to avoid the
        thundering-herd problem on the first request after expiry.
        """
        async with self._token_lock:
            if (
                not force_refresh
                and self._cached_token
                and time.time() < self._token_expiry - 120
            ):
                return self._cached_token

            try:
                session = await self._ensure_session()
                async with session.post(
                    f"{self.base_url}/oauth2/token",
                    headers={"Content-Type": "application/x-www-form-urlencoded"},
                    data={
                        "grant_type": "client_credentials",
                        "appkey": self.app_key,
                        "appsecretkey": self.app_secret,
                        "scope": "oob",
                    },
                ) as response:
                    data = await response.json()
                    token = data.get("access_token")
                    expires_in = int(data.get("expires_in", 0))

                    if token:
                        self._cached_token = token
                        self._token_expiry = time.time() + expires_in
                        logger.info("[EBest] token refreshed (ttl=%ds)", expires_in)
                        return token
                    logger.error("[EBest] token issuance failed: %s", data)
                    return None
            except Exception as exc:
                logger.error("[EBest] auth_token error: %s", exc)
                return None

    @staticmethod
    def _is_token_error(result: dict[str, Any]) -> bool:
        rsp_cd = result.get("rsp_cd", "")
        rsp_msg = str(result.get("rsp_msg", ""))
        if rsp_cd in _TOKEN_ERROR_CODES:
            return True
        if any(kw in rsp_msg.lower() for kw in _TOKEN_ERROR_KEYWORDS):
            return True
        return False

    async def enforce_rate_limit(self, tr_code: str) -> None:
        """Reserve the next allowed slot per TR code, then sleep asynchronously.

        Never blocks the event loop — concurrent callers queue up fairly.
        """
        async with self._rate_lock:
            now = time.monotonic()
            slot = max(now, self._next_call_at.get(tr_code, 0.0))
            self._next_call_at[tr_code] = slot + MIN_CALL_INTERVAL
        wait = slot - now
        if wait > 0:
            await asyncio.sleep(wait)

    # ------------------------------------------------------------------
    # market data
    # ------------------------------------------------------------------

    async def stock_price(self, token: str, shcode: str) -> dict[str, Any]:
        """t1101 — current price snapshot."""
        path = "stock/market-data"
        body = {"t1101InBlock": {"shcode": shcode}}

        async def _do(tok: str) -> dict[str, Any]:
            headers = {
                "tr_cd": "t1101",
                "Content-Type": "application/json; charset=utf-8",
                "authorization": f"Bearer {tok}",
                "tr_cont": "N",
                "tr_cont_key": "",
            }
            await self.enforce_rate_limit("t1101")
            session = await self._ensure_session()
            async with session.post(
                f"{self.base_url}/{path}", headers=headers, json=body
            ) as r:
                return await r.json()

        try:
            result = await _do(token)
            if self._is_token_error(result):
                new_token = await self.auth_token(force_refresh=True)
                if new_token:
                    result = await _do(new_token)
            if "t1101OutBlock" in result:
                raw = result["t1101OutBlock"]
                block = dict(raw) if isinstance(raw, dict) else {}
                block["drate"] = _t1101_change_rate_percent(block)
                return block
            if result.get("rsp_cd") in _RATE_LIMIT_ERROR_CODES:
                logger.warning("[EBest] stock_price rate limited: %s", result)
                return {
                    "_rate_limited": True,
                    "_error_code": result.get("rsp_cd"),
                    "_error_message": result.get("rsp_msg"),
                }
            logger.error("[EBest] stock_price response: %s", result)
            return {}
        except Exception as exc:
            logger.error("[EBest] stock_price error: %s", exc)
            return {}

    async def fetch_candle_data_tr(
        self,
        token: str,
        shcode: str,
        ncnt: int = 1,
        qrycnt: int = 2000,
        nday: str = "0",
        sdate: str = "",
        stime: str = "",
        edate: str = "",
        etime: str = "",
    ) -> dict[str, Any]:
        """t8452 — 분봉 차트 조회 (n분 단위, auto token retry).

        t8452 는 edate(종료일)가 비어 있으면 rsp_cd=00000 이면서도 빈 결과를
        돌려준다. edate 미지정 시 당일(KST)로 채우고, nday="0" 으로 qrycnt 만큼의
        최근 봉을 끌어온다.
        """
        path = "stock/chart"
        if not edate:
            edate = datetime.now(KST).strftime("%Y%m%d")
        body = {
            "t8452InBlock": {
                "shcode": shcode,
                "ncnt": ncnt,
                "qrycnt": qrycnt,
                "nday": nday,
                "sdate": sdate,
                "stime": stime,
                "edate": edate,
                "etime": etime,
                "cts_date": "",
                "cts_time": "",
                "comp_yn": "N",
                "exchgubun": "K",
            }
        }

        async def _do(tok: str) -> dict[str, Any]:
            headers = {
                "content-type": "application/json; charset=utf-8",
                "authorization": f"Bearer {tok}",
                "tr_cd": "t8452",
                "tr_cont": "N",
                "tr_cont_key": "",
            }
            await self.enforce_rate_limit("t8452")
            session = await self._ensure_session()
            async with session.post(
                f"{self.base_url}/{path}", headers=headers, json=body
            ) as response:
                return await response.json()

        try:
            result = await _do(token)
            if self._is_token_error(result):
                logger.warning(
                    "[EBest] fetch_candle_data_tr token error → refreshing",
                )
                new_token = await self.auth_token(force_refresh=True)
                if new_token:
                    result = await _do(new_token)
            if result.get("rsp_cd") in _RATE_LIMIT_ERROR_CODES:
                logger.warning("[EBest] fetch_candle_data_tr rate limited → retry")
                await asyncio.sleep(MIN_CALL_INTERVAL)
                result = await _do(token)
            return result
        except Exception as exc:
            logger.error("[EBest] fetch_candle_data_tr error: %s", exc)
            return {}

    async def fetch_daily_candles(
        self,
        token: str,
        shcode: str,
        qrycnt: int = 60,
    ) -> list[dict[str, Any]]:
        """t8451 — 일봉 차트 조회 (날짜 오름차순 OHLCV 행 목록 반환)."""
        path = "stock/chart"
        now_kst = datetime.now(KST)
        edate = now_kst.strftime("%Y%m%d")
        sdate = (now_kst.date() - timedelta(days=qrycnt * 2)).strftime("%Y%m%d")
        body = {
            "t8451InBlock": {
                "shcode": shcode,
                "gubun": "2",
                "qrycnt": qrycnt,
                "sdate": sdate,
                "edate": edate,
                "cts_date": "",
                "comp_yn": "N",
                "sujung": "Y",
                "exchgubun": "K",
            }
        }

        async def _do(tok: str) -> dict[str, Any]:
            headers = {
                "content-type": "application/json; charset=utf-8",
                "authorization": f"Bearer {tok}",
                "tr_cd": "t8451",
                "tr_cont": "N",
                "tr_cont_key": "",
            }
            await self.enforce_rate_limit("t8451")
            session = await self._ensure_session()
            async with session.post(
                f"{self.base_url}/{path}", headers=headers, json=body
            ) as response:
                return await response.json()

        try:
            result = await _do(token)
            if self._is_token_error(result):
                logger.warning("[EBest] fetch_daily_candles token error → refreshing")
                new_token = await self.auth_token(force_refresh=True)
                if new_token:
                    result = await _do(new_token)
            if result.get("rsp_cd") in _RATE_LIMIT_ERROR_CODES:
                logger.warning("[EBest] fetch_daily_candles rate limited → retry")
                await asyncio.sleep(MIN_CALL_INTERVAL)
                result = await _do(token)
        except Exception as exc:
            logger.error("[EBest] fetch_daily_candles error: %s", exc)
            return []

        rows: list[dict[str, Any]] = []
        for item in result.get("t8451OutBlock1") or []:
            try:
                rows.append(
                    {
                        "date": str(item.get("date", "")).strip(),
                        "open": float(item.get("open", 0) or 0),
                        "high": float(item.get("high", 0) or 0),
                        "low": float(item.get("low", 0) or 0),
                        "close": float(item.get("close", 0) or 0),
                        "volume": float(item.get("jdiff_vol", 0) or 0),
                    }
                )
            except (TypeError, ValueError):
                continue
        rows = [r for r in rows if r["date"] and r["high"] > 0 and r["low"] > 0]
        rows.sort(key=lambda r: r["date"])
        return rows

    # ------------------------------------------------------------------
    # real order management
    # ------------------------------------------------------------------

    async def place_order(
        self,
        token: str,
        code: str,
        side: str,
        quantity: int,
        price: float,
        price_type: str = "00",
    ) -> dict[str, Any]:
        """CSPAT00601 — 현물 매수/매도 주문.

        side: 'buy' | 'sell'
        price_type: '00'=지정가, '03'=시장가
        계좌는 OAuth 토큰으로 식별되므로 AcntNo/InptPwd 불필요.
        """
        isu_no = f"A{code}" if not code.startswith("A") else code
        bns_tp = "2" if side == "buy" else "1"
        path = "stock/order"
        body = {
            "CSPAT00601InBlock1": {
                "IsuNo": isu_no,
                "OrdQty": quantity,
                "OrdPrc": int(price),
                "BnsTpCode": bns_tp,
                "OrdprcPtnCode": price_type,
                "MgntrnCode": "000",
                "LoanDt": "",
                "OrdCndiTpCode": "0",
            }
        }

        async def _do(tok: str) -> dict[str, Any]:
            headers = {
                "content-type": "application/json; charset=utf-8",
                "authorization": f"Bearer {tok}",
                "tr_cd": "CSPAT00601",
                "tr_cont": "N",
                "tr_cont_key": "",
            }
            await self.enforce_rate_limit("CSPAT00601")
            session = await self._ensure_session()
            async with session.post(
                f"{self.base_url}/{path}", headers=headers, json=body
            ) as r:
                return await r.json()

        try:
            result = await _do(token)
            if self._is_token_error(result):
                new_token = await self.auth_token(force_refresh=True)
                if new_token:
                    result = await _do(new_token)
            return result
        except Exception as exc:
            logger.error("[EBest] place_order error: %s", exc)
            return {"rsp_cd": "ERR", "rsp_msg": str(exc)}

    async def get_account_balance(self, token: str) -> dict[str, Any]:
        """t0424 — 주식 잔고2 조회 (보유 종목 목록 + 예수금)."""
        path = "stock/accno"
        body = {
            "t0424InBlock": {
                "prcgb": "1",
                "chegb": "2",
                "dangb": "N",
                "charge": "N",
                "cts_expcode": "",
            }
        }

        async def _do(tok: str) -> dict[str, Any]:
            headers = {
                "content-type": "application/json; charset=utf-8",
                "authorization": f"Bearer {tok}",
                "tr_cd": "t0424",
                "tr_cont": "N",
                "tr_cont_key": "",
            }
            await self.enforce_rate_limit("t0424")
            session = await self._ensure_session()
            async with session.post(
                f"{self.base_url}/{path}", headers=headers, json=body
            ) as r:
                return await r.json()

        try:
            result = await _do(token)
            if self._is_token_error(result):
                new_token = await self.auth_token(force_refresh=True)
                if new_token:
                    result = await _do(new_token)
            return result
        except Exception as exc:
            logger.error("[EBest] get_account_balance error: %s", exc)
            return {}
