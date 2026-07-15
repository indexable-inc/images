"""Kayak backend (kayak.com/hotels).

Kayak is a meta-search aggregator with a clean date-in-path URL but aggressive
bot protection, so this backend is best-effort and degrades to zero rows (rather
than failing the whole comparison) when challenged. Each result card is a
``resultInner`` element whose lines are name / "Compare" / neighborhood / rating
(/10) / "<label> (<reviews>)", with the nightly price elsewhere in the card.
"""

from __future__ import annotations

import re

from ..cdp import CDP
from ..models import Hotel, money
from . import Query

_BOT = ("security check", "please verify", "are you a human", "px-captcha")
_CARD = '[class*="resultInner"]'

# Query.sort -> Kayak's ?sort= token, so a rating/recommended search draws its
# rows from the requested ranking instead of always the cheapest page.
_SORTS = {"price": "price_a", "recommended": "rank_a", "rating": "userrating_b"}

_EXTRACT = r"""
(() => {
  const seen = new Set(); const out = [];
  document.querySelectorAll('[class*="resultInner"]').forEach(c => {
    const it = c.innerText || '';
    const price = (it.match(/\$[\d,]+/) || [])[0];
    if (!price) return;
    const lines = it.split('\n').map(s => s.trim()).filter(Boolean);
    const name = lines[0];
    if (!name || name.length < 3 || /^\$/.test(name)) return;
    const area = lines.find(l => /,/.test(l) && !/\$/.test(l) && l !== name) || null;
    const rating = (it.match(/\b(\d\.\d)\b/) || [])[1] || null;   // Kayak is /10
    const reviews = (it.match(/\(([\d,]+)\)/) || [])[1] || null;
    const a = c.querySelector('a[href]');
    const key = name + '|' + price;
    if (seen.has(key)) return; seen.add(key);
    out.push({ name, price, area, rating, reviews, url: a ? a.href : null });
  });
  return out;
})()
"""


def _slug(where: str) -> str:
    # Kayak's path expects a hyphenated place token; it resolves the city to its
    # own id (e.g. "San Francisco" -> ".../San-Francisco-c13852/...").
    return re.sub(r"\s+", "-", where.strip())


class KayakBackend:
    name = "kayak"
    supported_amenities: frozenset[str] = frozenset()

    def _url(self, q: Query) -> str:
        return (
            f"https://www.kayak.com/hotels/{_slug(q.where)}/"
            f"{q.check_in.isoformat()}/{q.check_out.isoformat()}/{q.adults}adults"
            f"?sort={_SORTS.get(q.sort, 'price_a')}"
        )

    async def search(self, cdp: CDP, q: Query) -> tuple[list[Hotel], int | None]:
        await cdp.navigate(self._url(q))
        await cdp.wait_for(_CARD, timeout=q.timeout, bot_markers=_BOT)
        await cdp.scroll_until(_CARD, limit=q.limit)
        cards = await cdp.eval(_EXTRACT) or []
        hotels = [
            Hotel.from_card(
                self.name,
                {
                    "name": c.get("name"),
                    "nightly": money(c.get("price")),
                    "area": c.get("area"),
                    "rating": c.get("rating"),
                    "reviews": c.get("reviews"),
                    "url": c.get("url"),
                },
            )
            for c in cards
        ]
        return hotels[: q.limit], None

    async def amenities(self, cdp: CDP, q: Query) -> list[tuple[str, int]]:
        return []
