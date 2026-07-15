"""Network-free unit tests for the `hotels` module: parsing, URL building, schema,
and the cross-site matching. The live scrapers need a real browser, so they are
exercised manually, not here."""

from __future__ import annotations

import asyncio
import datetime as dt

import polars as pl
import pytest

import hotels
from hotels.backends import Query, get
from hotels.backends.booking import _stay_total
from hotels.backends.expedia import ExpediaBackend
from hotels.models import HOTEL_SCHEMA, Hotel, SearchResult, money


def test_money() -> None:
    assert money("$1,234") == 1234
    assert money("178") == 178
    assert money(None) is None
    assert money("") is None
    assert money("free") is None


def test_hotel_from_card_cleans_name_and_prices() -> None:
    h = Hotel.from_card(
        "expedia",
        {
            "name": "Photo gallery for HI San Francisco Downtown Hostel",
            "nightly": "$178",
            "total": "$207",
            "prices": ["$207", "$178"],
            "rating": "9.0",
            "reviews": "440",
        },
    )
    assert h.site == "expedia"
    assert h.name == "HI San Francisco Downtown Hostel"  # caption prefix stripped
    assert h.price_per_night == 178
    assert h.total_price == 207
    assert h.rating == 9.0
    assert h.review_count == 440


def test_hotel_nightly_falls_back_to_cheapest_price() -> None:
    h = Hotel.from_card("booking", {"name": "X", "prices": ["$300", "$250", "$280"]})
    assert h.price_per_night == 250  # min of the seen prices when nightly absent


def test_searchresult_df_schema_and_empty() -> None:
    empty = SearchResult(
        where="SF", check_in=dt.date(2026, 6, 17), check_out=dt.date(2026, 6, 18),
        adults=2, sort="price",
    )
    assert empty.df.columns == list(HOTEL_SCHEMA)
    assert empty.df.height == 0

    res = SearchResult(
        where="SF", check_in=dt.date(2026, 6, 17), check_out=dt.date(2026, 6, 18),
        adults=2, sort="price",
        hotels=[
            Hotel(site="expedia", name="B", price_per_night=300),
            Hotel(site="expedia", name="A", price_per_night=178),
        ],
    )
    # Sorted cheapest first.
    assert res.df["name"].to_list() == ["A", "B"]
    assert res.df.schema["badges"] == pl.List(pl.Utf8)


def test_expedia_url_maps_amenities_to_verified_tokens() -> None:
    q = Query(
        where="San Francisco", check_in=dt.date(2026, 6, 17),
        check_out=dt.date(2026, 6, 18), adults=2, sort="price",
        amenities=("washer", "kitchen"),
    )
    url = ExpediaBackend()._url(q)
    assert "destination=San%20Francisco" in url
    assert "startDate=2026-06-17" in url
    assert "endDate=2026-06-18" in url
    assert "sort=PRICE_LOW_TO_HIGH" in url
    assert "amenities=WASHER_DRYER" in url
    assert "amenities=KITCHEN_KITCHENETTE" in url


def test_unknown_amenity_rejected() -> None:
    with pytest.raises(ValueError, match="unknown amenity"):
        hotels._validate_amenities(["jacuzzi"])  # not a canonical key


def test_comparison_matches_aligns_same_hotel_across_sites() -> None:
    result = SearchResult(
        where="SF", check_in=dt.date(2026, 6, 17), check_out=dt.date(2026, 6, 18),
        adults=2, sort="price",
        hotels=[
            Hotel(site="expedia", name="HI San Francisco Downtown Hostel", price_per_night=178),
            Hotel(site="booking", name="HI San Francisco Downtown Hostel", price_per_night=146),
            Hotel(site="google", name="Some Unique Inn", price_per_night=99),
        ],
    )
    cmp = hotels.Comparison(result=result)
    matches = cmp.matches
    # Two clusters: the shared HI hostel + the unique inn.
    assert matches.height == 2
    hi = matches.filter(pl.col("name").str.contains("HI San Francisco")).row(0, named=True)
    assert hi["expedia"] == 178
    assert hi["booking"] == 146
    assert hi["cheapest_site"] == "booking"
    assert hi["cheapest_price"] == 146
    assert hi["n_sites"] == 2


def test_all_sites_registered() -> None:
    assert set(hotels.ALL_SITES) == {"expedia", "google", "booking", "kayak"}


def test_booking_discounted_price_takes_lowest() -> None:
    # "$300 $250" must not become 300250; take the discounted (lowest) amount.
    assert _stay_total("$300 $250") == 250
    assert _stay_total("US$1,234") == 1234
    assert _stay_total(None) is None


def test_only_expedia_filters_amenities() -> None:
    assert "washer" in get("expedia").supported_amenities
    for site in ("google", "booking", "kayak"):
        assert get(site).supported_amenities == frozenset()


def test_run_skips_backend_that_cannot_filter() -> None:
    # A backend that can't honor the amenity is reported as an error, not run
    # (no browser is contacted), so a filtered comparison never includes it.
    q = Query(
        where="SF", check_in=dt.date(2026, 6, 17), check_out=dt.date(2026, 6, 18),
        amenities=("washer",),
    )
    res = asyncio.run(hotels._run("google", q, hotels.DEFAULT_ENDPOINT))
    assert res["hotels"] == []
    assert "cannot filter by" in res["error"]
