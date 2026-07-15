"""Typed boundary + shared schema for hotel search across backends.

Every backend parses its site's rendered cards into the SAME :class:`Hotel`
model, so results from Expedia, Google, Booking and Kayak stack into one polars
frame and can be compared. Parsing lives here (``Hotel.from_card``) so a backend
only has to hand over a loose dict scraped from the page.
"""

from __future__ import annotations

import datetime as _dt
import re
from typing import Any

import polars as pl
from pydantic import BaseModel, ConfigDict

# Canonical, site-independent amenity keys a caller filters on. Each backend maps
# these to its own on-site filter (see backends/expedia._AMENITY_TOKENS), so
# `amenities=["washer"]` means the same thing everywhere; a backend that can't
# filter declares no support and is skipped for filtered searches.
CANONICAL_AMENITIES: tuple[str, ...] = (
    "washer",          # in-unit washer / washer-dryer / laundry in room
    "kitchen",
    "pool",
    "gym",
    "parking",
    "pet_friendly",
    "wifi",
    "air_conditioning",
    "breakfast",
    "hot_tub",
)

# The polars schema for a result row, fixed so an empty search still yields a
# frame with the right columns and dtypes.
HOTEL_SCHEMA: dict[str, Any] = {
    "site": pl.Utf8,
    "name": pl.Utf8,
    "price_per_night": pl.Int64,
    "total_price": pl.Int64,
    "currency": pl.Utf8,
    "area": pl.Utf8,
    "rating": pl.Float64,
    "review_count": pl.Int64,
    "url": pl.Utf8,
    "badges": pl.List(pl.Utf8),
}


def money(text: str | None) -> int | None:
    """``"$1,234"`` / ``"1234"`` -> ``1234``; unparseable -> ``None``."""
    if text is None:
        return None
    digits = re.sub(r"[^\d]", "", str(text))
    return int(digits) if digits else None


def as_date(value: str | _dt.date) -> _dt.date:
    """Accept a ``date`` or an ISO ``"YYYY-MM-DD"`` string."""
    if isinstance(value, _dt.date):
        return value
    return _dt.date.fromisoformat(str(value))


def first_float(text: str | None) -> float | None:
    if text in (None, ""):
        return None
    try:
        return float(text)
    except (TypeError, ValueError):
        return None


class Hotel(BaseModel):
    """One search-result property, normalized across every backend."""

    model_config = ConfigDict(extra="ignore")  # forgive each site's DOM churn

    site: str
    name: str
    price_per_night: int | None = None
    total_price: int | None = None
    currency: str = "USD"
    area: str | None = None
    rating: float | None = None
    review_count: int | None = None
    url: str | None = None
    badges: list[str] = []

    @classmethod
    def from_card(cls, site: str, raw: dict[str, Any]) -> Hotel:
        """Coerce one backend's scraped card dict into a ``Hotel``.

        Expected (all optional) raw keys: ``name``, ``nightly``, ``total``,
        ``prices`` (list of "$N" strings), ``rating``, ``reviews``, ``area``,
        ``url``, ``badges``, ``currency``.
        """
        name = (raw.get("name") or "").strip()
        # Expedia uses "Photo gallery for <name>" as a figure caption that can win
        # the title lookup; strip it back to the real name.
        name = re.sub(r"^Photo gallery for\s+", "", name).strip()

        prices = [p for p in (money(p) for p in raw.get("prices") or []) if p]
        nightly = money(raw.get("nightly")) or (min(prices) if prices else None)

        return cls.model_validate(
            {
                "site": site,
                "name": name,
                "price_per_night": nightly,
                "total_price": money(raw.get("total")),
                "currency": raw.get("currency") or "USD",
                "area": (raw.get("area") or None),
                "rating": first_float(raw.get("rating")),
                "review_count": money(raw.get("reviews")),
                "url": (raw.get("url") or None),
                "badges": [b for b in (raw.get("badges") or []) if b],
            }
        )


class SearchResult(BaseModel):
    """A search on one or more sites: echoed query + the parsed :class:`Hotel` rows."""

    model_config = ConfigDict(extra="ignore")

    where: str
    check_in: _dt.date
    check_out: _dt.date
    adults: int
    sort: str
    amenities: list[str] = []
    sites: list[str] = []
    total_found: dict[str, int] = {}  # per-site result-count header, when shown
    errors: dict[str, str] = {}  # per-site failure reason (challenge, timeout, ...)
    hotels: list[Hotel] = []

    @property
    def df(self) -> pl.DataFrame:
        """One row per hotel, fixed schema, cheapest first (empty -> typed empty)."""
        if not self.hotels:
            return pl.DataFrame(schema=HOTEL_SCHEMA)
        frame = pl.DataFrame(
            [h.model_dump() for h in self.hotels], schema_overrides=HOTEL_SCHEMA
        ).select(list(HOTEL_SCHEMA))
        return frame.sort("price_per_night", nulls_last=True)

    def _repr_html_(self) -> str:
        return self.df._repr_html_()
