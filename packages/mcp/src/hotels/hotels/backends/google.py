"""Google Hotels backend (google.com/travel).

Google Hotels has the most obfuscated, frequently-rotated DOM of the four and
encodes its date state in an opaque token, so this backend is best-effort: it
reads the rendered price cards and degrades to zero rows (rather than crashing
the whole comparison) when Google changes its markup or challenges the session.
"""

from __future__ import annotations

from urllib.parse import quote

from ..cdp import CDP
from ..models import Hotel, money
from . import Query

_BOT = ("unusual traffic", "sorry/index", "recaptcha")
# Google rotates class names, so anchor the card on the stable structural fact:
# a results container whose entries each show a "$N" price and a heading.
_CARD = 'a[href*="/travel/hotels/"]'

_EXTRACT = r"""
(() => {
  const seen = new Set(); const out = [];
  document.querySelectorAll('a[href*="/travel/hotels/"]').forEach(a => {
    const card = a.closest('c-wiz, li, div[role="listitem"]') || a;
    const it = (card.innerText || '').trim();
    const price = (it.match(/\$[\d,]+/) || [])[0] || null;
    if (!price) return;
    // The name is the first substantial line: has a letter, not a bare number,
    // price or rating fragment (Google sprinkles those as their own lines).
    const name = it.split('\n').map(s => s.trim())
      .find(l => l.length >= 4 && /[A-Za-z]/.test(l) && !/^\$/.test(l) && !/^\d+(\.\d+)?$/.test(l)) || null;
    if (!name) return;
    const rating = (it.match(/\b(\d\.\d)\b/) || [])[1] || null;
    const reviews = (it.match(/\(([\d,]+)\)/) || [])[1] || null;
    const key = name + '|' + price;
    if (seen.has(key)) return; seen.add(key);
    out.push({ name, price, rating, reviews, url: a.href });
  });
  return out;
})()
"""


class GoogleBackend:
    name = "google"
    supported_amenities: frozenset[str] = frozenset()

    def _url(self, q: Query) -> str:
        # Google honors checkin/checkout/occupancy on the travel search surface
        # most of the time; when it does not, prices are for the nearest default
        # stay. Pass adults so occupancy matches the other backends rather than
        # silently defaulting to 2.
        return (
            "https://www.google.com/travel/search?"
            f"q=hotels%20in%20{quote(q.where)}&"
            f"checkin={q.check_in.isoformat()}&checkout={q.check_out.isoformat()}&"
            f"adults={q.adults}"
        )

    async def search(self, cdp: CDP, q: Query) -> tuple[list[Hotel], int | None]:
        await cdp.navigate(self._url(q))
        await cdp.wait_for(_CARD, timeout=q.timeout, bot_markers=_BOT)
        await cdp.scroll_until(_CARD, limit=q.limit)
        cards = await cdp.eval(_EXTRACT) or []
        hotels: list[Hotel] = []
        for c in cards:
            # Google shows per-night by default; if it shows a total, the value is
            # still a useful cross-check. Treat the figure as nightly.
            nightly = money(c.get("price"))
            # Google rates out of 5; the other backends report out of 10, so
            # double it to keep the `rating` column on one scale.
            rating = c.get("rating")
            rating = str(round(float(rating) * 2, 1)) if rating else None
            hotels.append(
                Hotel.from_card(
                    self.name,
                    {
                        "name": c.get("name"),
                        "nightly": nightly,
                        "rating": rating,
                        "reviews": c.get("reviews"),
                        "url": c.get("url"),
                    },
                )
            )
        return hotels[: q.limit], None

    async def amenities(self, cdp: CDP, q: Query) -> list[tuple[str, int]]:
        return []
