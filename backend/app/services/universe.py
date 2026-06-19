"""Stock universe loaded from files/search_item.csv.

Columns: 종목(name), 코드(code), 시장(market), 현재가(price), 전일종가(prev_close),
등락률(change_pct), 거래량(volume). Price fields enable current-price filtering.
"""

import csv
import logging
from functools import lru_cache
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

# backend/app/services/universe.py -> project root is three parents up.
CSV_PATH = Path(__file__).resolve().parents[3] / "files" / "search_item.csv"


def _num(value: Any) -> float:
    s = str(value or "").strip().replace(",", "")
    if not s:
        return 0.0
    try:
        return float(s)
    except ValueError:
        return 0.0


@lru_cache
def load_universe() -> tuple[dict, ...]:
    rows: list[dict] = []
    if not CSV_PATH.exists():
        logger.warning("universe csv not found: %s", CSV_PATH)
        return tuple(rows)
    with CSV_PATH.open(encoding="utf-8-sig") as f:
        reader = csv.DictReader(f)
        for r in reader:
            code = (r.get("코드") or "").strip()
            if not code:
                continue
            rows.append({
                "code": code,
                "name": (r.get("종목") or "").strip(),
                "market": (r.get("시장") or "").strip(),
                "price": _num(r.get("현재가")),
                "prev_close": _num(r.get("전일종가")),
                "change_pct": _num(r.get("등락률")),
                "volume": _num(r.get("거래량")),
            })
    return tuple(rows)


def filter_universe(
    market: str = "ALL",
    min_price: float | None = None,
    max_price: float | None = None,
    min_change: float | None = None,
    max_change: float | None = None,
) -> list[dict]:
    """Filter the universe by market and current-price / change-rate range."""
    rows = list(load_universe())
    if market and market != "ALL":
        rows = [r for r in rows if r["market"] == market]
    if min_price is not None:
        rows = [r for r in rows if r["price"] >= min_price]
    if max_price is not None:
        rows = [r for r in rows if r["price"] <= max_price]
    if min_change is not None:
        rows = [r for r in rows if r["change_pct"] >= min_change]
    if max_change is not None:
        rows = [r for r in rows if r["change_pct"] <= max_change]
    return rows


# Backwards-compatible alias (market-only filter).
def filter_market(market: str = "ALL") -> list[dict]:
    return filter_universe(market=market)


@lru_cache
def _by_code() -> dict:
    return {r["code"]: r for r in load_universe()}


def name_for(code: str) -> str:
    return _by_code().get(code, {}).get("name", "")


def info_for(code: str) -> dict:
    return _by_code().get(code, {})
