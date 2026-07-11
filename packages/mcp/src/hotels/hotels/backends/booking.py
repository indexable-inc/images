"""Booking.com backend: clean URL params, parse the property-card grid.

Amenity filtering is not implemented (Booking nests facility filters behind a
"Show all" dialog), so this backend declares no ``supported_amenities`` and is
skipped for amenity-filtered searches rather than returning unfiltered rows.
"""

from __future__ import annotations

import re
from urllib.parse import quote

from ..cdp import CDP
from ..models import Hotel, money
from . import Query

_CARD = '[data-testid="property-card"]'
_BOT = ("captcha", "are you a robot", "px-captcha")

_SORTS = {"price": "price", "recommended": "popularity", "rating": "review_score_and_price"}

_EXTRACT = r"""
(() => {
  const cards = [...document.querySelectorAll('[data-testid="property-card"]')];
  return cards.map(c => {
    const name = (c.querySelector('[data-testid="title"]') || {}).innerText || null;
    const priceEl = c.querySelector('[data-testid="price-and-discounted-price"]');
    const price = priceEl ? priceEl.innerText.trim() : null;
    const area = (c.querySelector('[data-testid="address"]') || {}).innerText || null;
    const rs = c.querySelector('[data-testid="review-score"]');
    const rt = rs ? rs.innerText : '';
    const rating = (rt.match(/(\d+[.,]\d)/) || [])[1] || null;
    const reviews = (rt.match(/([\d,]+)\s+reviews?/i) || [])[1] || null;
    const a = c.querySelector('a[href]');
    return { name, price, area, rating, reviews, url: a ? a.href : null };
  });
})()
"""


def _stay_total(price_text: str | None) -> int | None:
    """The current (discounted) stay price from a Booking price cell.

    The ``price-and-discounted-price`` cell can show both the struck-through
    original and the discounted price (e.g. ``"$300 $250"``); naively stripping
    non-digits would yield ``300250``. Take the lowest of the distinct amounts.

    Only dollar-denominated cells are parsed: `Hotel` rows are labeled USD, and
    a signed-in Booking session can override the URL's ``selected_currency`` with
    a saved preference (e.g. ``€250``), which must not be ranked as ``$250``.
    """
    if "$" not in (price_text or ""):
        return None
    amounts = [money(m) for m in re.findall(r"\d[\d,]+", price_text or "")]
    amounts = [a for a in amounts if a]
    return min(amounts) if amounts else None


class BookingBackend:
    name = "booking"
    supported_amenities: frozenset[str] = frozenset()

    def _url(self, q: Query) -> str:
        return (
            "https://www.booking.com/searchresults.html?"
            f"ss={quote(q.where)}&checkin={q.check_in.isoformat()}&"
            f"checkout={q.check_out.isoformat()}&group_adults={q.adults}&"
            f"no_rooms=1&group_children=0&order={_SORTS.get(q.sort, 'price')}&"
            # Every backend reports prices as USD (models.HOTEL_SCHEMA), and the
            # other extractors only match "$N" strings; pin Booking -- whose price
            # cell follows the session's saved currency -- to match.
            "selected_currency=USD"
        )

    async def search(self, cdp: CDP, q: Query) -> tuple[list[Hotel], int | None]:
        await cdp.navigate(self._url(q))
        await cdp.wait_for(_CARD, timeout=q.timeout, bot_markers=_BOT)
        await cdp.scroll_until(_CARD, limit=q.limit)
        cards = await cdp.eval(_EXTRACT) or []
        nights = max((q.check_out - q.check_in).days, 1)
        hotels: list[Hotel] = []
        for c in cards:
            total = _stay_total(c.get("price"))
            per_night = round(total / nights) if total else None
            hotels.append(
                Hotel.from_card(
                    self.name,
                    {
                        "name": c.get("name"),
                        "nightly": per_night,
                        "total": total,
                        "area": c.get("area"),
                        "rating": (c.get("rating") or "").replace(",", ".") or None,
                        "reviews": c.get("reviews"),
                        "url": c.get("url"),
                    },
                )
            )
        return hotels[: q.limit], None

    async def amenities(self, cdp: CDP, q: Query) -> list[tuple[str, int]]:
        return []  # Booking nests facilities behind a "Show all"; not enumerated yet
