"""Expedia backend: drive expedia.com Hotel-Search and parse the rendered cards.

Amenity filters are applied through the URL (`&amenities=WASHER_DRYER`), which is
far more robust than clicking the filter rail. Every token below was verified
against the live site (the unfiltered count is 245; a working token lowers it).
Expedia keeps rendering non-matching "Properties that don't match all your
filters" and out-of-area suggestions below the genuine matches, so extraction
stops at that divider.
"""

from __future__ import annotations

import asyncio
from urllib.parse import quote

from ..cdp import CDP
from ..models import Hotel
from . import Query

_CARD = '[data-stid="lodging-card-responsive"]'
_BOT = ("Bot or Not",)

_SORTS = {
    "price": "PRICE_LOW_TO_HIGH",
    "recommended": "RECOMMENDED",
    "rating": "REVIEW_RELEVANT",
}

# Canonical amenity key -> Expedia's URL filter token (all verified live).
_AMENITY_TOKENS = {
    "washer": "WASHER_DRYER",
    "kitchen": "KITCHEN_KITCHENETTE",
    "pool": "POOL",
    "gym": "GYM",
    "parking": "PARKING",
    "pet_friendly": "PETS",
    "wifi": "WIFI",
    "air_conditioning": "AIR_CONDITIONING",
    "breakfast": "FREE_BREAKFAST",
    "hot_tub": "HOT_TUB",
}

# One ordered pass over the results, stopping at the first divider heading: only
# cards before it are genuine matches for the applied filters.
_EXTRACT = r"""
(() => {
  const cont = document.querySelector('[data-stid="property-listing-results"]')
            || document.querySelector('main') || document.body;
  const DIVIDER = /don.?t match all your filters|propert(?:y|ies) (?:nearby|outside)|expand your search|see more propert|other propert|search outside/i;
  const cards = [];
  const walker = document.createTreeWalker(cont, NodeFilter.SHOW_ELEMENT);
  let n;
  while ((n = walker.nextNode())) {
    if (/^H[1-6]$/.test(n.tagName) && DIVIDER.test(n.innerText || '')) break;
    if (!(n.matches && n.matches('[data-stid="lodging-card-responsive"]'))) continue;
    const it = n.innerText || '';
    let name = null;
    const t = n.querySelector('[data-stid="content-hotel-title"]');
    if (t) name = t.innerText.trim();
    if (!name) for (const h of n.querySelectorAll('h3')) {
      const x = (h.innerText || '').trim();
      if (x && !/^Photo gallery for/i.test(x)) { name = x; break; }
    }
    const nightly = (it.match(/\$([\d,]+)\s*(?:\n|\s)*nightly/i) || [])[1] || null;
    const total = (it.match(/\$([\d,]+)\s*(?:\n|\s)*total/i) || [])[1] || null;
    const prices = [...new Set((it.match(/\$[\d,]+/g) || []))];
    const rating = (it.match(/(\d{1,2}(?:\.\d)?)\s*\/\s*10/) || it.match(/\b(\d\.\d)\b/) || [])[1] || null;
    const reviews = (it.match(/([\d,]+)\s+reviews?/i) || [])[1] || null;
    const a = n.querySelector('a[href]');
    let badges = [...n.querySelectorAll('[data-stid*="badge"], .uitk-badge')]
      .map(b => (b.innerText || '').trim())
      .filter(b => b && !/out of 10/i.test(b) && !/^\d(?:\.\d)?$/.test(b));
    cards.push({ name, nightly, total, prices, rating, reviews, area: null, url: a ? a.href : null, badges });
  }
  return cards;
})()
"""


class ExpediaBackend:
    name = "expedia"
    supported_amenities = frozenset(_AMENITY_TOKENS)

    def _url(self, q: Query) -> str:
        url = (
            "https://www.expedia.com/Hotel-Search?"
            f"destination={quote(q.where)}&"
            f"startDate={q.check_in.isoformat()}&endDate={q.check_out.isoformat()}&"
            f"adults={q.adults}&sort={_SORTS.get(q.sort, 'PRICE_LOW_TO_HIGH')}"
        )
        # Repeat the param per amenity (Expedia's multi-select encoding).
        for a in q.amenities:
            token = _AMENITY_TOKENS.get(a)
            if token:
                url += f"&amenities={token}"
        return url

    async def search(self, cdp: CDP, q: Query) -> tuple[list[Hotel], int | None]:
        await cdp.navigate(self._url(q))
        await cdp.wait_for(_CARD, timeout=q.timeout, bot_markers=_BOT)
        await asyncio.sleep(2.0)  # let the (possibly filtered) result list settle
        # Scroll to load up to `limit` cards even when filtered: a common amenity
        # (wifi/pool/parking) still has many genuine matches, and the extractor
        # stops at the "doesn't match all filters" divider, so loading more never
        # pulls suggestion cards into the result.
        await cdp.scroll_until(_CARD, limit=q.limit)
        cards = await cdp.eval(_EXTRACT) or []
        hotels = [Hotel.from_card(self.name, c) for c in cards][: q.limit]
        return hotels, len(hotels)

    async def amenities(self, cdp: CDP, q: Query) -> list[tuple[str, int]]:
        await cdp.navigate(self._url(q))
        await cdp.wait_for(_CARD, timeout=q.timeout, bot_markers=_BOT)
        # Amenity checkboxes live low in the filter rail; scroll to render them
        # and expand any "See more" before reading the labels + counts.
        for _ in range(4):
            await cdp.eval("window.scrollBy(0, window.innerHeight)")
            await asyncio.sleep(0.4)
        await cdp.eval(
            "[...document.querySelectorAll('button')]"
            ".filter(b=>/see more/i.test(b.innerText||''))"
            ".forEach(b=>{try{b.click()}catch(e){}})"
        )
        await asyncio.sleep(0.8)
        raw = await cdp.eval(
            r"""
            (() => {
              const seen = new Set(); const out = [];
              document.querySelectorAll('label,span,div').forEach(el => {
                const r = el.getBoundingClientRect();
                if (r.width === 0 || r.height === 0) return;
                const t = (el.innerText || '').trim();
                const m = t.match(/^([A-Za-z][A-Za-z &/'-]+?)\s*\((\d+)\)$/);
                if (m && !seen.has(m[1])) { seen.add(m[1]); out.push([m[1], parseInt(m[2])]); }
              });
              return out;
            })()
            """
        )
        return [(n, int(c)) for n, c in (raw or [])]
