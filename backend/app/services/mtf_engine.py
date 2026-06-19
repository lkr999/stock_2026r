"""Multi-timeframe confluence scoring (spec section 9-3)."""

import logging

from app.services.pattern_detector import PatternDetector
from app.services.timeframe import MTF_GROUPS, Timeframe

logger = logging.getLogger(__name__)


class MTFEngine:
    def __init__(self, fetcher, detector: PatternDetector) -> None:
        self._fetcher = fetcher
        self._detector = detector

    async def score(self, token: str, shcode: str, base_tf: Timeframe, pattern_type: str) -> float:
        """Fraction of upper timeframes that show the same-direction signal."""
        upper_tfs = MTF_GROUPS.get(base_tf, [])
        if not upper_tfs:
            return 0.5  # neutral when no higher timeframe exists
        hits = 0
        for tf in upper_tfs:
            try:
                candles = await self._fetcher.fetch(token, shcode, tf)
                results = self._detector.scan(candles, tf, min_confidence=0.5)
            except Exception as exc:
                logger.warning("MTF fetch failed %s %s: %s", shcode, tf, exc)
                continue
            if any(r.pattern_type == pattern_type for r in results):
                hits += 1
        return hits / len(upper_tfs)
