"""Search and cross-compare hotels across sites by driving your running browser.

Bundled into the ix-mcp interpreter like ``browser`` and ``x``, so a session can
``import hotels`` with no install step. Hotel sites challenge scripted/headless
browsers, so this does NOT launch one: it drives the browser you already have
open and signed in, over the Chrome DevTools Protocol (the standard debug port
9222), reusing that browser's real, trusted fingerprint. Each backend
(``expedia``, ``google``, ``booking``, ``kayak``) parses its site's rendered
cards into the same :class:`~hotels.models.Hotel`, so results stack into one
``polars`` DataFrame and compare directly.

    import hotels

    # one site -> a typed SearchResult whose .df is a polars frame, cheapest first
    res = await hotels.search(
        "San Francisco", check_in="YYYY-MM-DD", check_out="YYYY-MM-DD",
        amenities=["washer"], sort="price",
    )
    res.df

    # cross-compare every site at once
    cmp = await hotels.compare(
        "San Francisco", check_in="YYYY-MM-DD", check_out="YYYY-MM-DD",
        amenities=["washer"],
    )
    cmp.df        # all rows, tagged by `site`, cheapest first
    cmp.matches   # the same hotel aligned across sites, with the cheapest site
    cmp.errors    # any site that failed (bot wall, layout change) -> reason

    # discover the filterable amenities for a site
    await hotels.amenities("San Francisco", check_in="YYYY-MM-DD", check_out="YYYY-MM-DD")

``amenities`` are canonical, site-independent keys (``hotels.CANONICAL_AMENITIES``
-- e.g. ``"washer"``, ``"kitchen"``, ``"pool"``); each backend maps them to its
own on-site filter. A backend that gets challenged or finds nothing contributes
zero rows and an entry in ``cmp.errors`` instead of failing the whole search.

Transport note: the bundled :mod:`browser` module (Playwright over CDP) is the
intended shared abstraction, but its pinned driver currently cannot handshake
with very recent Chrome builds, so this speaks raw CDP through a small internal
seam (:mod:`hotels.cdp`) that can be swapped for :mod:`browser` once realigned.
"""

from __future__ import annotations

import asyncio
import datetime as _dt
import difflib
import re
from collections.abc import Sequence
from typing import Any

import polars as pl
from pydantic import BaseModel, ConfigDict

from . import backends
from .backends import Query
from .cdp import DEFAULT_ENDPOINT, CDP
from .models import (
    CANONICAL_AMENITIES,
    Hotel,
    SearchResult,
    as_date,
)

__all__ = [
    "CANONICAL_AMENITIES",
    "DEFAULT_ENDPOINT",
    "Comparison",
    "Hotel",
    "SearchResult",
    "amenities",
    "compare",
    "search",
]

__version__ = "0.1.0"

# All registered backends, in display order; the default set for `compare`.
ALL_SITES: tuple[str, ...] = tuple(backends.REGISTRY)

# Words dropped before fuzzy-matching a name across sites (generic hotel noise),
# so "HI San Francisco Downtown Hostel" and "HI San Francisco Downtown" align.
_STOPWORDS = {
    "hotel", "hotels", "the", "a", "an", "inn", "suites", "suite", "and",
    "by", "at", "resort", "motel", "hostel",
}


def _norm(name: str) -> str:
    tokens = re.sub(r"[^a-z0-9 ]", " ", (name or "").lower()).split()
    return " ".join(t for t in tokens if t not in _STOPWORDS)


async def _run(site: str, q: Query, endpoint: str) -> dict[str, Any]:
    """Drive one backend in its own tab; never raise -- report errors as data."""
    backend = backends.get(site)
    # A backend that cannot apply a requested amenity must NOT contribute
    # unfiltered rows (they would pollute a filtered comparison and could be
    # ranked cheapest). Skip it and report why.
    unsupported = [a for a in q.amenities if a not in backend.supported_amenities]
    if unsupported:
        return {"site": site, "hotels": [], "total": None,
                "error": f"cannot filter by {unsupported} on {site}"}
    try:
        async with CDP(endpoint) as cdp:
            hotels, total = await backend.search(cdp, q)
        return {"site": site, "hotels": hotels, "total": total, "error": None}
    except Exception as exc:
        # One site failing (bot wall, layout change) must not sink the rest.
        return {"site": site, "hotels": [], "total": None, "error": f"{type(exc).__name__}: {exc}"}


def _validate_amenities(amenities: Sequence[str]) -> tuple[str, ...]:
    bad = [a for a in amenities if a not in CANONICAL_AMENITIES]
    if bad:
        raise ValueError(
            f"unknown amenity {bad}; choose from {list(CANONICAL_AMENITIES)}"
        )
    return tuple(amenities)


async def _fanout(
    where: str, *, check_in: str | _dt.date, check_out: str | _dt.date,
    adults: int, sort: str, amenities: Sequence[str], limit: int,
    sites: Sequence[str], endpoint: str, timeout: float,
) -> tuple[Query, list[dict[str, Any]]]:
    q = Query(
        where=where, check_in=as_date(check_in), check_out=as_date(check_out),
        adults=adults, sort=sort, amenities=_validate_amenities(amenities),
        limit=limit, timeout=timeout,
    )
    results = await asyncio.gather(*(_run(s, q, endpoint) for s in sites))
    return q, list(results)


async def search(
    where: str, *, check_in: str | _dt.date, check_out: str | _dt.date,
    adults: int = 2, sort: str = "price", amenities: Sequence[str] = (),
    limit: int = 50, sites: Sequence[str] = ("expedia",),
    endpoint: str = DEFAULT_ENDPOINT, timeout: float = 30.0,
) -> SearchResult:
    """Search one or more ``sites`` and return a typed :class:`SearchResult`.

    ``check_in``/``check_out`` are ``date`` or ISO ``"YYYY-MM-DD"`` strings.
    ``sort`` is ``price`` (default), ``recommended`` or ``rating``. ``amenities``
    are canonical keys (see ``hotels.CANONICAL_AMENITIES``). Defaults to Expedia;
    pass ``sites=hotels.ALL_SITES`` for everything. ``.df`` is the polars frame,
    cheapest first, with a ``site`` column; ``.errors`` maps any failed site to
    its reason. Raises if EVERY requested site failed, so the common single-site
    call surfaces a dead browser / bot wall instead of a silently empty frame.
    """
    q, results = await _fanout(
        where, check_in=check_in, check_out=check_out, adults=adults, sort=sort,
        amenities=amenities, limit=limit, sites=sites, endpoint=endpoint, timeout=timeout,
    )
    hotels = [h for r in results for h in r["hotels"]]
    totals = {r["site"]: r["total"] for r in results if r["total"] is not None}
    errors = {r["site"]: r["error"] for r in results if r["error"]}
    if errors and len(errors) == len(results):
        raise RuntimeError(
            "hotel search failed on every site: "
            + "; ".join(f"{s}: {e}" for s, e in errors.items())
        )
    return SearchResult(
        where=where, check_in=q.check_in, check_out=q.check_out, adults=adults,
        sort=sort, amenities=list(q.amenities), sites=list(sites),
        total_found=totals, errors=errors, hotels=hotels,
    )


class Comparison(BaseModel):
    """A cross-site comparison: stacked rows plus the same hotel aligned across sites."""

    model_config = ConfigDict(arbitrary_types_allowed=True)

    result: SearchResult
    errors: dict[str, str] = {}

    @property
    def df(self) -> pl.DataFrame:
        """Every hotel from every site, cheapest first (the ``site`` column tags each)."""
        return self.result.df

    @property
    def matches(self) -> pl.DataFrame:
        """One row per hotel matched across sites: a price column per site + the cheapest.

        Names are normalized and fuzzy-matched (only across different sites), so a
        property listed on several sites collapses to one row showing each site's
        nightly price and which site is cheapest. Single-site hotels appear too.
        """
        hotels = self.result.hotels
        if not hotels:
            return pl.DataFrame()
        clusters: list[dict[str, Hotel]] = []
        keys: list[str] = []
        for h in sorted(hotels, key=lambda x: (x.price_per_night is None, x.price_per_night or 0)):
            nk = _norm(h.name)
            placed = False
            for i, key in enumerate(keys):
                if h.site in clusters[i]:
                    continue  # one entry per site per cluster
                if difflib.SequenceMatcher(None, nk, key).ratio() >= 0.82:
                    clusters[i][h.site] = h
                    placed = True
                    break
            if not placed:
                clusters.append({h.site: h})
                keys.append(nk)

        sites = [s for s in ALL_SITES if any(s in c for c in clusters)]
        rows: list[dict[str, Any]] = []
        for cluster in clusters:
            best = min(
                (h for h in cluster.values() if h.price_per_night is not None),
                key=lambda h: h.price_per_night,
                default=None,
            )
            row: dict[str, Any] = {
                "name": next(iter(cluster.values())).name,
                "area": next((h.area for h in cluster.values() if h.area), None),
                "cheapest_site": best.site if best else None,
                "cheapest_price": best.price_per_night if best else None,
                "n_sites": len(cluster),
            }
            for s in sites:
                row[f"{s}"] = cluster[s].price_per_night if s in cluster else None
            rows.append(row)
        schema = {
            "name": pl.Utf8, "area": pl.Utf8, "cheapest_site": pl.Utf8,
            "cheapest_price": pl.Int64, "n_sites": pl.Int64,
            **dict.fromkeys(sites, pl.Int64),
        }
        return pl.DataFrame(rows, schema_overrides=schema).select(list(schema)).sort(
            "cheapest_price", nulls_last=True
        )

    def _repr_html_(self) -> str:
        frame = self.matches
        if frame.height:
            return frame._repr_html_()
        return self.df._repr_html_()


async def compare(
    where: str, *, check_in: str | _dt.date, check_out: str | _dt.date,
    adults: int = 2, sort: str = "price", amenities: Sequence[str] = (),
    limit: int = 50, sites: Sequence[str] = ALL_SITES,
    endpoint: str = DEFAULT_ENDPOINT, timeout: float = 30.0,
) -> Comparison:
    """Search every site in ``sites`` concurrently and cross-compare the prices.

    Returns a :class:`Comparison`: ``.df`` stacks all rows (tagged by ``site``,
    cheapest first), ``.matches`` aligns the same hotel across sites with the
    cheapest one flagged, and ``.errors`` maps any failed site to its reason.
    """
    q, results = await _fanout(
        where, check_in=check_in, check_out=check_out, adults=adults, sort=sort,
        amenities=amenities, limit=limit, sites=sites, endpoint=endpoint, timeout=timeout,
    )
    hotels = [h for r in results for h in r["hotels"]]
    totals = {r["site"]: r["total"] for r in results if r["total"] is not None}
    errors = {r["site"]: r["error"] for r in results if r["error"]}
    if errors and len(errors) == len(results):
        # Every site failed (dead browser, bot wall everywhere): a silently empty
        # Comparison would read as "no hotels", so fail loudly like search().
        raise RuntimeError(
            "hotel comparison failed on every site: "
            + "; ".join(f"{s}: {e}" for s, e in errors.items())
        )
    result = SearchResult(
        where=where, check_in=q.check_in, check_out=q.check_out, adults=adults,
        sort=sort, amenities=list(q.amenities), sites=list(sites),
        total_found=totals, errors=errors, hotels=hotels,
    )
    return Comparison(result=result, errors=errors)


async def amenities(
    where: str, *, check_in: str | _dt.date, check_out: str | _dt.date,
    adults: int = 2, site: str = "expedia", endpoint: str = DEFAULT_ENDPOINT,
    timeout: float = 30.0,
) -> pl.DataFrame:
    """The on-site filterable amenities for ``site`` and their live match counts.

    Returns a polars frame with ``amenity`` and ``count`` columns. (Expedia is the
    only backend that enumerates these today; others return empty.)
    """
    backend = backends.get(site)
    schema = {"amenity": pl.Utf8, "count": pl.Int64}
    if not backend.supported_amenities:
        # This site exposes no filterable amenity catalog, so return the
        # documented empty frame without touching the browser (opening a CDP tab
        # here would fail whenever no debug browser is listening).
        return pl.DataFrame([], schema=schema, orient="row")
    q = Query(
        where=where, check_in=as_date(check_in), check_out=as_date(check_out),
        adults=adults, timeout=timeout,
    )
    async with CDP(endpoint) as cdp:
        options = await backend.amenities(cdp, q)
    return pl.DataFrame(options or [], schema=schema, orient="row")
