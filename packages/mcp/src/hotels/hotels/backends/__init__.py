"""Backend protocol + registry: one scraper per hotel site, one shared shape.

A backend turns a :class:`Query` into normalized :class:`~hotels.models.Hotel`
rows by driving the running browser (over :class:`~hotels.cdp.CDP`). The package
API (:func:`hotels.search` / :func:`hotels.compare`) fans out across the
registered backends and stacks the results.
"""

from __future__ import annotations

import dataclasses
import datetime as _dt
from typing import Protocol, runtime_checkable

from ..cdp import CDP
from ..models import Hotel


@dataclasses.dataclass(frozen=True)
class Query:
    """A site-independent hotel search request."""

    where: str
    check_in: _dt.date
    check_out: _dt.date
    adults: int = 2
    sort: str = "price"  # price | recommended | rating
    amenities: tuple[str, ...] = ()  # canonical keys (models.CANONICAL_AMENITIES)
    limit: int = 50
    timeout: float = 30.0


@runtime_checkable
class Backend(Protocol):
    """One hotel site. ``search`` returns (rows, total_found-or-None)."""

    name: str
    # Canonical amenity keys this backend can actually filter on. A search that
    # requests an amenity outside this set is skipped (and reported) for the
    # backend rather than returning unfiltered rows -- otherwise a filtered
    # cross-site comparison could mark an unfiltered property as cheapest.
    supported_amenities: frozenset[str]

    async def search(self, cdp: CDP, q: Query) -> tuple[list[Hotel], int | None]:
        """Search the site and return (parsed rows, total_found-or-None)."""

    async def amenities(self, cdp: CDP, q: Query) -> list[tuple[str, int]]:
        """Filterable amenities + live match counts (best-effort; may be empty)."""


def _build_registry() -> dict[str, Backend]:
    from .booking import BookingBackend
    from .expedia import ExpediaBackend
    from .google import GoogleBackend
    from .kayak import KayakBackend

    backends: tuple[Backend, ...] = (
        ExpediaBackend(),
        GoogleBackend(),
        BookingBackend(),
        KayakBackend(),
    )
    return {b.name: b for b in backends}


REGISTRY: dict[str, Backend] = _build_registry()


def get(name: str) -> Backend:
    try:
        return REGISTRY[name]
    except KeyError:
        raise ValueError(
            f"unknown site {name!r}; choose from {sorted(REGISTRY)}"
        ) from None
