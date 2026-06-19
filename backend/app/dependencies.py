"""Shared FastAPI dependencies (access app.state singletons)."""

from fastapi import HTTPException, Request


def get_ebest(request: Request):
    return request.app.state.ebest


def get_fetcher(request: Request):
    return request.app.state.fetcher


def get_detector(request: Request):
    return request.app.state.detector


async def get_token(request: Request) -> str:
    """eBest 인증 토큰을 반환한다. 토큰 없이는 데이터를 조회할 수 없다.

    토큰 발급 실패(키 미설정/네트워크 오류) 시 503 을 반환한다 — 절대로
    가짜/합성 데이터로 대체하지 않는다.
    """
    token = await request.app.state.ebest.auth_token() or ""
    if not token:
        raise HTTPException(
            503,
            "eBest 인증 실패 — 토큰을 발급할 수 없습니다. .env 의 "
            "EBEST_APP_KEY/EBEST_APP_SECRET 와 네트워크를 확인하세요.",
        )
    return token
