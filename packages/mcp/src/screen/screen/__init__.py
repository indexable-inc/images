"""Native macOS screen capture and cursor control for the ix-mcp interpreter.

Bundled into the pinned interpreter the same way `tui` and `playwright` are, so
every session can `import screen` with no install step. Where `tui` drives PTYs
and `playwright` drives browsers, `screen` drives the native desktop: it reads
the framebuffer and the mouse, and posts synthetic input through CoreGraphics.

    import screen
    img = screen.capture()          # full virtual desktop as a PIL.Image
    print(img.size)                 # auto-rendered inline by python_eval/exec

    region = screen.capture(screen.Rect(0, 0, 400, 300))  # a sub-rectangle
    where = screen.cursor()         # current pointer, a screen.Point
    screen.move(100, 200)           # warp the cursor (no permission needed)
    screen.click(100, 200)          # synthetic click (needs Accessibility, see below)
    screen.drag(100, 200, 300, 400) # press, move, release

Capture returns a `PIL.Image` in RGB. Because the worker renders any returned
PIL image inline, returning the result of `capture()` from a cell shows the
screenshot directly. NumPy users can call `capture_ndarray()` for an
`(H, W, 3)` uint8 array.

macOS Accessibility (TCC) permission

Reading the screen and the cursor, and *warping* the cursor with `move()`, work
with no special permission. Posting synthetic mouse input (`click`, `drag`, and
`mouse_down`/`mouse_up`) requires the host process to be trusted for
Accessibility under System Settings > Privacy & Security > Accessibility. The OS
silently drops posted events from an untrusted process, which makes GUI
interaction tests look like they pass while nothing happens. To avoid that
silent no-op, the synthetic-input helpers check `accessibility_trusted()` first
and raise `AccessibilityNotTrusted` with the offending process name when the
permission is missing, rather than posting an event the OS will discard.

    if not screen.accessibility_trusted():
        # grant the *terminal/host* running ix-mcp Accessibility access, then
        # restart it; the grant is per-binary and does not transfer across a
        # rebuilt store path.
        ...

This module is macOS-only: it wraps CoreGraphics through pyobjc `Quartz` (an
Apple-maintained binding) for capture and input, and probes the documented
`AXIsProcessTrusted()` C API through ctypes for the permission check. Importing
on a non-Darwin platform raises `RuntimeError`.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import sys
from dataclasses import dataclass

__all__ = [
    "AccessibilityNotTrusted",
    "Point",
    "Rect",
    "Size",
    "accessibility_trusted",
    "capture",
    "capture_ndarray",
    "click",
    "cursor",
    "drag",
    "mouse_down",
    "mouse_up",
    "move",
    "screen_size",
]

if sys.platform != "darwin":
    raise RuntimeError(
        "screen: native capture and cursor control are macOS-only "
        f"(running on {sys.platform!r}). Use playwright for browsers or tui for terminals."
    )

# pyobjc `Quartz` is the Apple-maintained CoreGraphics binding; it owns capture,
# cursor read, and synthetic-event posting. Import errors should name the
# dependency so a stripped environment is diagnosable rather than mysterious.
try:
    import Quartz  # noqa: N813  (Apple framework module name)
except ImportError as exc:  # pragma: no cover - environment wiring
    raise RuntimeError(
        "screen: pyobjc `Quartz` is required but not importable; the ix-mcp "
        "interpreter is built with it, so this usually means a non-bundled "
        "interpreter is in use."
    ) from exc

from PIL import Image


@dataclass(frozen=True, slots=True)
class Point:
    """A screen coordinate in points, origin at the top-left of the main display."""

    x: float
    y: float


@dataclass(frozen=True, slots=True)
class Size:
    """A width/height pair in pixels."""

    width: int
    height: int


@dataclass(frozen=True, slots=True)
class Rect:
    """A capture rectangle in points: (x, y) top-left plus width/height."""

    x: float
    y: float
    width: float
    height: float


class AccessibilityNotTrusted(PermissionError):
    """Raised when synthetic input is attempted without Accessibility permission.

    macOS silently discards posted events from an untrusted process, so the
    input helpers fail loudly with this instead of posting an event that does
    nothing. Grant the host process Accessibility access (System Settings >
    Privacy & Security > Accessibility), then restart it.
    """


# `AXIsProcessTrusted` lives in the ApplicationServices/HIServices framework,
# which pyobjc does not package in nixpkgs. It is a stable, documented C API
# (returns Boolean, no arguments), so a one-symbol ctypes probe is the smallest
# correct surface. https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrusted
def _load_application_services() -> ctypes.CDLL:
    path = (
        ctypes.util.find_library("ApplicationServices")
        or "/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices"
    )
    lib = ctypes.cdll.LoadLibrary(path)
    lib.AXIsProcessTrusted.restype = ctypes.c_bool
    lib.AXIsProcessTrusted.argtypes = []
    return lib


_APP_SERVICES = _load_application_services()


def accessibility_trusted() -> bool:
    """Whether this process may post synthetic input (Accessibility / TCC).

    Reflects `AXIsProcessTrusted()`. False means `click`, `drag`, `mouse_down`,
    and `mouse_up` will raise `AccessibilityNotTrusted` rather than emit an event
    the OS discards. Warping the cursor with `move()` and reading the screen do
    not require this.
    """

    return bool(_APP_SERVICES.AXIsProcessTrusted())


def _require_accessibility() -> None:
    if not accessibility_trusted():
        proc = sys.argv[0] if sys.argv and sys.argv[0] else sys.executable
        raise AccessibilityNotTrusted(
            "screen: synthetic input requires macOS Accessibility permission, "
            f"which {proc!r} does not have. macOS would silently discard the "
            "event. Grant this process Accessibility access under System "
            "Settings > Privacy & Security > Accessibility, then restart it. "
            "Check with screen.accessibility_trusted()."
        )


def screen_size() -> Size:
    """Pixel size of the main display."""

    main = Quartz.CGMainDisplayID()
    bounds = Quartz.CGDisplayBounds(main)
    return Size(int(bounds.size.width), int(bounds.size.height))


def _cgimage_to_pil(image: object) -> Image.Image:
    """Convert a CGImage to an RGB PIL image, honoring the row stride.

    CoreGraphics rows are padded to `bytesPerRow`, which is wider than
    `width * 4` on most displays, so the raw buffer must be unpacked with the
    real stride or the image shears. The pixel order is BGRA premultiplied;
    dropping alpha to RGB is the common, lossless-for-opaque-desktop choice.
    """

    width = Quartz.CGImageGetWidth(image)
    height = Quartz.CGImageGetHeight(image)
    bytes_per_row = Quartz.CGImageGetBytesPerRow(image)
    provider = Quartz.CGImageGetDataProvider(image)
    data = bytes(Quartz.CGDataProviderCopyData(provider))
    pil = Image.frombuffer("RGBA", (width, height), data, "raw", "BGRA", bytes_per_row, 1)
    return pil.convert("RGB")


def capture(region: Rect | None = None) -> Image.Image:
    """Capture the screen (or a sub-rectangle) as an RGB `PIL.Image`.

    With no argument, captures the full virtual desktop across all displays.
    Pass a `Rect` in points to capture a region. The returned image is rendered
    inline when it is the value of a python_eval/python_exec cell.
    """

    if region is None:
        image = Quartz.CGDisplayCreateImage(Quartz.CGMainDisplayID())
    else:
        cg_rect = Quartz.CGRectMake(region.x, region.y, region.width, region.height)
        image = Quartz.CGWindowListCreateImage(
            cg_rect,
            Quartz.kCGWindowListOptionOnScreenOnly,
            Quartz.kCGNullWindowID,
            Quartz.kCGWindowImageDefault,
        )
    if image is None:
        raise RuntimeError(
            "screen: capture returned no image. On recent macOS, reading the "
            "screen also needs Screen Recording permission (System Settings > "
            "Privacy & Security > Screen Recording) for the host process."
        )
    return _cgimage_to_pil(image)


def capture_ndarray(region: Rect | None = None):
    """Capture as an `(H, W, 3)` uint8 NumPy array (RGB).

    Convenience for pixel math and image diffing; wraps `capture()`.
    """

    import numpy as np

    return np.asarray(capture(region))


def cursor() -> Point:
    """Current mouse pointer location as a `Point` (top-left origin)."""

    location = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
    return Point(location.x, location.y)


def move(x: float, y: float) -> None:
    """Warp the cursor to (x, y). Needs no Accessibility permission."""

    Quartz.CGWarpMouseCursorPosition(Quartz.CGPointMake(x, y))


def _post_mouse(event_type: int, x: float, y: float, button: int) -> None:
    event = Quartz.CGEventCreateMouseEvent(None, event_type, Quartz.CGPointMake(x, y), button)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, event)


def mouse_down(x: float, y: float) -> None:
    """Press the left mouse button at (x, y). Requires Accessibility permission."""

    _require_accessibility()
    _post_mouse(Quartz.kCGEventLeftMouseDown, x, y, Quartz.kCGMouseButtonLeft)


def mouse_up(x: float, y: float) -> None:
    """Release the left mouse button at (x, y). Requires Accessibility permission."""

    _require_accessibility()
    _post_mouse(Quartz.kCGEventLeftMouseUp, x, y, Quartz.kCGMouseButtonLeft)


def click(x: float, y: float) -> None:
    """Left-click at (x, y): press then release. Requires Accessibility permission.

    Raises `AccessibilityNotTrusted` if the process is not trusted, instead of
    posting an event macOS would silently discard.
    """

    _require_accessibility()
    _post_mouse(Quartz.kCGEventLeftMouseDown, x, y, Quartz.kCGMouseButtonLeft)
    _post_mouse(Quartz.kCGEventLeftMouseUp, x, y, Quartz.kCGMouseButtonLeft)


def drag(x1: float, y1: float, x2: float, y2: float) -> None:
    """Drag from (x1, y1) to (x2, y2): press, move, release.

    Requires Accessibility permission; raises `AccessibilityNotTrusted`
    otherwise.
    """

    _require_accessibility()
    _post_mouse(Quartz.kCGEventLeftMouseDown, x1, y1, Quartz.kCGMouseButtonLeft)
    _post_mouse(Quartz.kCGEventLeftMouseDragged, x2, y2, Quartz.kCGMouseButtonLeft)
    _post_mouse(Quartz.kCGEventLeftMouseUp, x2, y2, Quartz.kCGMouseButtonLeft)
