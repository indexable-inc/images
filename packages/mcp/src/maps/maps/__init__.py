"""Native macOS maps and location for the ix-mcp interpreter.

Bundled like ``screen``/``imessage`` so every session can ``import maps`` on
Darwin with no install step. Where ``screen`` drives the desktop and ``imessage``
reads Messages, ``maps`` reaches Apple's location stack: it searches for places
near a point (MapKit ``MKLocalSearch``) and turns an address into coordinates and
back (CoreLocation ``CLGeocoder``). Results come back as polars frames, so
returning one from a cell renders inline.

    import maps
    await maps.nearby("coffee", 37.3349, -122.009)   # places near a point
    await maps.geocode("Apple Park, Cupertino, CA")    # address -> lat/lng
    await maps.reverse_geocode(37.3349, -122.009)      # lat/lng -> address

(There is no current-location helper: CoreLocation only authorizes a signed .app
bundle, and ix-mcp runs as a bare daemon, so a ``CLLocationManager`` fix can
never be granted here. Geocode a known address instead.)

Why a run-loop bridge (the non-obvious part)
--------------------------------------------
Each of these APIs is asynchronous and delivers its result through a completion
handler on the **main thread / main dispatch queue**. The kernel's single asyncio
event loop owns the main thread but services it as an asyncio loop, not as a
CoreFoundation ``CFRunLoop`` -- so a naive call's handler never fires and the
await hangs. ``_await_handler`` bridges the two: it kicks off the Cocoa call, then
cooperatively drains the main run loop in short, bounded slices between ``await``s
until the handler lands, marshalling the result back to the coroutine. Each slice
is a couple milliseconds, so a co-running coroutine on the shared kernel sees only
a small scheduling hitch, never the indefinite block a plain ``CFRunLoopRun()``
would cause.

This bridge is deliberately general -- it knows nothing about maps -- so a future
CoreLocation/EventKit/Contacts helper can reuse it. The moment a second consumer
lands it should be promoted to a shared module; until then it lives here.

No API key, no account, no cost: this is the on-device Apple stack, not a REST
service; the calls need only network access.

macOS-only: importing on a non-Darwin platform raises ``RuntimeError``.
"""

from __future__ import annotations

import asyncio
import math
import sys
import time
from collections.abc import Callable
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import polars as pl

__all__ = [
    "MapsError",
    "geocode",
    "nearby",
    "reverse_geocode",
]

if sys.platform != "darwin":
    raise RuntimeError(
        "maps: Apple's location stack is macOS-only "
        f"(running on {sys.platform!r}). For places elsewhere use a REST API "
        "(OpenStreetMap Overpass, Apple Maps Server API) over httpx."
    )


class MapsError(RuntimeError):
    """A maps/location call failed (Cocoa error, timeout, or denied permission)."""


# pyobjc bindings. ``objc`` and ``Foundation`` come from pyobjc-core (always
# present on Darwin in this interpreter); ``CoreLocation`` is bundled explicitly
# (see the mcp package). MapKit has no nixpkgs binding, so its classes are loaded
# from the system framework at import time. Name the dependency on failure so a
# stripped environment is diagnosable rather than mysterious.
try:
    import objc
    from Foundation import NSDate, NSRunLoop
    import CoreLocation
except ImportError as exc:  # pragma: no cover - environment wiring
    raise RuntimeError(
        "maps: pyobjc `objc`/`Foundation`/`CoreLocation` are required but not "
        "importable; the ix-mcp interpreter bundles them on Darwin, so this "
        "usually means a non-bundled interpreter is in use."
    ) from exc


_RUNLOOP_MODE = "kCFRunLoopDefaultMode"
# One drain slice: how long runMode:beforeDate: may block the (shared) event-loop
# thread while waiting for main-loop work before returning to yield to asyncio.
# runMode returns early once it processes a source, but while merely waiting it
# blocks to the limit date -- so this is the worst-case scheduling hitch a
# co-running coroutine on the shared kernel sees during a maps call. Kept to a
# couple ms (not tens) to keep that hitch small; the await between slices then
# hands the loop back to other coroutines.
_DRAIN_SLICE = 0.002
_YIELD = 0.005


def _load_mapkit() -> tuple[object, bool]:
    """Return ``(mapkit_namespace, from_binding)``.

    Prefers the bundled pyobjc ``MapKit`` binding (the production path on Darwin),
    which ships the completion-handler block metadata; otherwise loads the classes
    straight from the system framework bundle for a dev interpreter that lacks the
    binding. Either way the namespace exposes ``MKLocalSearchRequest`` /
    ``MKLocalSearch``. ``from_binding`` says which path was taken, so the caller
    knows whether the block metadata is already present.
    """
    try:
        import MapKit  # the bundled pyobjc binding (carries block metadata)

        return MapKit, True
    except ImportError:
        import types

        ns: dict[str, object] = {}
        objc.loadBundle("MapKit", ns, bundle_path="/System/Library/Frameworks/MapKit.framework")
        return types.SimpleNamespace(**ns), False


_MAPKIT: object | None = None


def _mapkit() -> object:
    """MapKit namespace, loaded once and cached.

    The bundled binding already carries the metadata for
    ``startWithCompletionHandler:`` (verified: it returns results with no manual
    registration). Only a raw ``loadBundle`` fallback -- a dev interpreter without
    the binding -- needs the metadata registered, else the block raises "no
    signature available".
    """
    global _MAPKIT
    if _MAPKIT is None:
        mapkit, from_binding = _load_mapkit()
        if not from_binding:
            # pyobjc block metadata lists the implicit block pointer at index 0,
            # then the real (response, error) arguments at 1 and 2; the Python
            # handler receives only the latter two.
            objc.registerMetaDataForSelector(
                b"MKLocalSearch",
                b"startWithCompletionHandler:",
                {
                    "arguments": {
                        2: {
                            "callable": {
                                "retval": {"type": b"v"},
                                "arguments": {0: {"type": b"^v"}, 1: {"type": b"@"}, 2: {"type": b"@"}},
                            }
                        }
                    }
                },
            )
        _MAPKIT = mapkit
    return _MAPKIT


async def _await_handler(start: Callable[[Callable[[object], None]], object], *, timeout: float) -> object:
    """Run a Cocoa completion-handler call and await its result on the kernel loop.

    ``start(done)`` must kick off the asynchronous Cocoa call, arranging for its
    completion handler (which Cocoa runs on the main thread) to eventually call
    ``done(value)`` -- where ``value`` is the result, or an ``Exception`` to
    raise. We then drain the main run loop in bounded slices, yielding to asyncio
    between each, until ``done`` fires or ``timeout`` elapses.

    This is the one place that touches the run loop; everything else builds on it.
    """
    box: dict[str, object] = {}

    def done(value: object) -> None:
        box["value"] = value

    start(done)
    runloop = NSRunLoop.currentRunLoop()
    deadline = time.monotonic() + timeout
    while "value" not in box and time.monotonic() < deadline:
        # Drain ready main-loop / main-queue work without blocking long...
        runloop.runMode_beforeDate_(_RUNLOOP_MODE, NSDate.dateWithTimeIntervalSinceNow_(_DRAIN_SLICE))
        # ...then hand the loop back to other coroutines.
        await asyncio.sleep(_YIELD)
    if "value" not in box:
        raise MapsError(f"timed out after {timeout:.0f}s waiting for a Cocoa completion handler")
    value = box["value"]
    if isinstance(value, BaseException):
        raise value
    return value


def _region(lat: float, lng: float, radius_m: float) -> tuple[tuple[float, float], tuple[float, float]]:
    """An MKCoordinateRegion (center, span) covering ``radius_m`` around a point.

    Returned as the nested tuple pyobjc encodes into the struct from the runtime
    method signature. Span is the full width/height, so it is twice the radius;
    longitude degrees shrink with latitude (``cos``), latitude degrees do not.
    """
    lat_delta = (2 * radius_m) / 111_320.0
    lng_delta = (2 * radius_m) / (111_320.0 * max(math.cos(math.radians(lat)), 1e-6))
    return ((lat, lng), (lat_delta, lng_delta))


def _str(value: object) -> str | None:
    """A Cocoa string-ish value as a Python str, or None."""
    return None if value is None else str(value)


def _placemark_row(placemark: object) -> dict[str, object]:
    """Common address/coordinate columns from a CL/MK placemark."""
    location = placemark.location()
    coord = location.coordinate() if location is not None else None
    return {
        "name": _str(placemark.name()),
        "latitude": coord.latitude if coord is not None else None,
        "longitude": coord.longitude if coord is not None else None,
        "thoroughfare": _str(placemark.thoroughfare()),
        "sub_thoroughfare": _str(placemark.subThoroughfare()),
        "locality": _str(placemark.locality()),
        "administrative_area": _str(placemark.administrativeArea()),
        "postal_code": _str(placemark.postalCode()),
        "country": _str(placemark.country()),
        "iso_country_code": _str(placemark.ISOcountryCode()),
        # Placemarks carry no URL; MKMapItem results get one in _mapitem_row.
        "url": None,
    }


def _mapitem_row(item: object) -> dict[str, object]:
    """One MKMapItem (a search result) as a row: placemark fields plus POI meta."""
    row = _placemark_row(item.placemark())
    # MKMapItem.name is the business/POI name; the placemark's name is often just
    # the street, so prefer the map item's.
    row["name"] = _str(item.name())
    category = item.pointOfInterestCategory()
    # POI categories read as "MKPOICategoryCafe"; strip the prefix for a tidy tag.
    row["category"] = _str(category).removeprefix("MKPOICategory") if category is not None else None
    row["phone"] = _str(item.phoneNumber())
    nsurl = item.url()
    row["url"] = _str(nsurl.absoluteString()) if nsurl is not None else None
    return row


async def nearby(
    query: str,
    latitude: float,
    longitude: float,
    *,
    radius_m: float = 2000.0,
    timeout: float = 20.0,
) -> pl.DataFrame:
    """Search for places matching ``query`` near a coordinate (MapKit).

    Returns one row per result with name, latitude, longitude, category, phone,
    url, and address fields, ordered as MapKit ranks them. ``radius_m`` sizes the
    search region around the center (a hint, not a hard cutoff). No API key or
    account is required -- this is Apple's on-device search.

        await maps.nearby("coffee", 37.3349, -122.009)
        await maps.nearby("hardware store", 40.7128, -74.0060, radius_m=5000)
    """
    import polars as pl

    mapkit = _mapkit()
    request = mapkit.MKLocalSearchRequest.alloc().init()
    request.setNaturalLanguageQuery_(query)
    request.setRegion_(_region(latitude, longitude, radius_m))
    search = mapkit.MKLocalSearch.alloc().initWithRequest_(request)

    def start(done: Callable[[object], None]) -> None:
        def handler(response: object, error: object) -> None:
            if error is not None:
                done(MapsError(f"nearby({query!r}) failed: {error}"))
            else:
                done([_mapitem_row(item) for item in response.mapItems()])

        search.startWithCompletionHandler_(handler)

    rows = await _await_handler(start, timeout=timeout)
    return pl.DataFrame(rows, schema=_nearby_schema(pl))


async def geocode(address: str, *, timeout: float = 20.0) -> pl.DataFrame:
    """Turn an address or place name into coordinates (CoreLocation CLGeocoder).

    Returns one row per match (usually one) with latitude, longitude, and the
    resolved address fields.

        await maps.geocode("1600 Amphitheatre Parkway, Mountain View, CA")
    """
    import polars as pl

    geocoder = CoreLocation.CLGeocoder.alloc().init()

    def start(done: Callable[[object], None]) -> None:
        def handler(placemarks: object, error: object) -> None:
            if error is not None:
                done(MapsError(f"geocode({address!r}) failed: {error}"))
            else:
                done([_placemark_row(pm) for pm in (placemarks or [])])

        geocoder.geocodeAddressString_completionHandler_(address, handler)

    rows = await _await_handler(start, timeout=timeout)
    return pl.DataFrame(rows, schema=_placemark_schema(pl))


async def reverse_geocode(latitude: float, longitude: float, *, timeout: float = 20.0) -> pl.DataFrame:
    """Turn a coordinate into nearby address(es) (CoreLocation CLGeocoder).

    Returns one row per match with the resolved address fields and the snapped
    coordinate.

        await maps.reverse_geocode(37.3349, -122.009)
    """
    import polars as pl

    geocoder = CoreLocation.CLGeocoder.alloc().init()
    location = CoreLocation.CLLocation.alloc().initWithLatitude_longitude_(latitude, longitude)

    def start(done: Callable[[object], None]) -> None:
        def handler(placemarks: object, error: object) -> None:
            if error is not None:
                done(MapsError(f"reverse_geocode({latitude}, {longitude}) failed: {error}"))
            else:
                done([_placemark_row(pm) for pm in (placemarks or [])])

        geocoder.reverseGeocodeLocation_completionHandler_(location, handler)

    rows = await _await_handler(start, timeout=timeout)
    return pl.DataFrame(rows, schema=_placemark_schema(pl))


# Explicit schemas keep column dtypes stable even when a frame is empty (no
# results) or a column is all-null for one query but populated for the next.
# Built lazily from the caller's polars so the module imports without polars at
# top level (matching the other bundled helpers).
def _placemark_schema(pl: object) -> dict[str, object]:
    return {
        "name": pl.String,
        "latitude": pl.Float64,
        "longitude": pl.Float64,
        "thoroughfare": pl.String,
        "sub_thoroughfare": pl.String,
        "locality": pl.String,
        "administrative_area": pl.String,
        "postal_code": pl.String,
        "country": pl.String,
        "iso_country_code": pl.String,
        "url": pl.String,
    }


def _nearby_schema(pl: object) -> dict[str, object]:
    return {**_placemark_schema(pl), "category": pl.String, "phone": pl.String}
