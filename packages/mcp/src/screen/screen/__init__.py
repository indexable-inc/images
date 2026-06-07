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

    screen.write("hello world")     # type text into the focused app
    screen.press("a", "cmd")        # a key or chord (Cmd+A); also press("return")
    screen.frontmost()              # app control: which app is active
    screen.activate("Safari")       # launch / activate / list (apps()) / terminate

Keyboard input (`write`, `press`, `key_down`/`key_up`) posts synthetic events
and so needs Accessibility permission, like the mouse helpers. App control
(`apps`, `frontmost`, `launch`, `activate`, `terminate`) goes through NSWorkspace
and needs no special permission.

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
    "App",
    "Point",
    "Rect",
    "Size",
    "accessibility_trusted",
    "activate",
    "apps",
    "capture",
    "capture_ndarray",
    "click",
    "cursor",
    "drag",
    "frontmost",
    "key_down",
    "key_up",
    "launch",
    "mouse_down",
    "mouse_up",
    "move",
    "press",
    "screen_size",
    "terminate",
    "write",
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


# macOS ANSI virtual key codes for keys that have no character to `write()`:
# named keys (Return, Tab, arrows, ...) and the letter/digit codes a chord like
# Cmd+A needs. Layout-independent text typing does not use these \u2014 `write()`
# injects Unicode directly \u2014 so this table only needs the keys you press by
# name or combine with a modifier.
_KEYS: dict[str, int] = {
    # letters (US ANSI positions; the character a key produces depends on the
    # active layout, but a chord like Cmd+A keys off the physical position)
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8,
    "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17,
    "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
    # digits
    "1": 18, "2": 19, "3": 20, "4": 21, "5": 23, "6": 22, "7": 26, "8": 28, "9": 25, "0": 29,
    # whitespace / editing
    "return": 36, "enter": 36, "tab": 48, "space": 49, "delete": 51, "backspace": 51,
    "escape": 53, "esc": 53, "forward_delete": 117,
    # navigation
    "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
    "left": 123, "right": 124, "down": 125, "up": 126,
    # function row
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98,
    "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
    # common punctuation
    "minus": 27, "equal": 24, "leftbracket": 33, "rightbracket": 30, "backslash": 42,
    "semicolon": 41, "quote": 39, "comma": 43, "period": 47, "slash": 44, "grave": 50,
}

_MODIFIERS: dict[str, int] = {
    "command": Quartz.kCGEventFlagMaskCommand,
    "cmd": Quartz.kCGEventFlagMaskCommand,
    "shift": Quartz.kCGEventFlagMaskShift,
    "option": Quartz.kCGEventFlagMaskAlternate,
    "opt": Quartz.kCGEventFlagMaskAlternate,
    "alt": Quartz.kCGEventFlagMaskAlternate,
    "control": Quartz.kCGEventFlagMaskControl,
    "ctrl": Quartz.kCGEventFlagMaskControl,
}


def _resolve_key(name: str) -> int:
    code = _KEYS.get(name.lower())
    if code is None:
        raise ValueError(
            f"screen: unknown key {name!r}. Use a named key ({', '.join(sorted(_KEYS))}) "
            "for press()/key_down()/key_up(), or write() to type arbitrary text."
        )
    return code


def _modifier_flags(modifiers: tuple[str, ...]) -> int:
    flags = 0
    for mod in modifiers:
        flag = _MODIFIERS.get(mod.lower())
        if flag is None:
            raise ValueError(
                f"screen: unknown modifier {mod!r}. Use one of: {', '.join(sorted(_MODIFIERS))}."
            )
        flags |= flag
    return flags


def _post_key(keycode: int, down: bool, flags: int) -> None:
    event = Quartz.CGEventCreateKeyboardEvent(None, keycode, down)
    if flags:
        Quartz.CGEventSetFlags(event, flags)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, event)


def write(text: str) -> None:
    """Type ``text`` into the focused app, character by character.

    Injects each character as a Unicode keystroke, so it is independent of the
    active keyboard layout and handles symbols and non-ASCII. Use this for text;
    use `press` for named keys (Return, Tab) and shortcuts. Requires
    Accessibility permission.
    """

    _require_accessibility()
    for char in text:
        for down in (True, False):
            event = Quartz.CGEventCreateKeyboardEvent(None, 0, down)
            Quartz.CGEventKeyboardSetUnicodeString(event, len(char), char)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, event)


def press(name: str, *modifiers: str) -> None:
    """Press and release a named key, optionally as a chord with modifiers.

        screen.press("return")
        screen.press("a", "cmd")        # Cmd+A (select all)
        screen.press("left", "shift")   # extend selection left

    ``name`` is a key from the named set or a single letter/digit; modifiers are
    any of command/cmd, shift, option/opt/alt, control/ctrl. Requires
    Accessibility permission.
    """

    _require_accessibility()
    keycode = _resolve_key(name)
    flags = _modifier_flags(modifiers)
    _post_key(keycode, True, flags)
    _post_key(keycode, False, flags)


def key_down(name: str, *modifiers: str) -> None:
    """Press and hold a key (no release), for chords built up by hand. Pair with
    `key_up`. Requires Accessibility permission."""

    _require_accessibility()
    _post_key(_resolve_key(name), True, _modifier_flags(modifiers))


def key_up(name: str, *modifiers: str) -> None:
    """Release a key held with `key_down`. Requires Accessibility permission."""

    _require_accessibility()
    _post_key(_resolve_key(name), False, _modifier_flags(modifiers))


@dataclass(frozen=True, slots=True)
class App:
    """A running application: its display name, bundle id, pid, and whether it is
    the active (frontmost) app."""

    name: str
    bundle_id: str | None
    pid: int
    active: bool


def _workspace():
    """The shared NSWorkspace. AppKit is imported lazily so the common
    capture/input paths do not pay for it and a stripped environment fails only
    when app control is actually used."""

    from AppKit import NSWorkspace

    return NSWorkspace.sharedWorkspace()


def _as_app(running) -> App:
    return App(
        running.localizedName(),
        running.bundleIdentifier(),
        int(running.processIdentifier()),
        bool(running.isActive()),
    )


def _find_running(app: str):
    """The running application whose bundle id or display name matches ``app``
    (case-insensitive), or None. App control acts on a running instance, so this
    is how a name/bundle-id argument resolves to one."""

    key = app.lower()
    for running in _workspace().runningApplications():
        if (running.bundleIdentifier() or "").lower() == key or (running.localizedName() or "").lower() == key:
            return running
    return None


def apps() -> list[App]:
    """Every running application as an `App`. App control (`activate`,
    `terminate`) takes a name or bundle id; this is how you discover them."""

    return [_as_app(running) for running in _workspace().runningApplications()]


def frontmost() -> App | None:
    """The active (frontmost) application, or None if there is none."""

    running = _workspace().frontmostApplication()
    return _as_app(running) if running is not None else None


def launch(app: str) -> App | None:
    """Launch an application by name or full path, or activate it if it is already
    running. Returns the running `App` once it appears (None if it has not
    registered yet \u2014 query `apps()`). Needs no Accessibility permission.
    """

    # NSWorkspace's synchronous replacement (openApplicationAtURL:...) is
    # completion-handler only; launchApplication_ stays the simplest correct
    # synchronous call and also foregrounds an already-running app.
    if not _workspace().launchApplication_(app):
        raise RuntimeError(
            f"screen: could not launch app {app!r}. Pass the app's name (e.g. "
            "'Safari') or its full .app path."
        )
    running = _find_running(app)
    return _as_app(running) if running is not None else None


def activate(app: str) -> None:
    """Bring a running application to the front by name or bundle id. Raises
    `LookupError` if it is not running (launch it first). Needs no Accessibility
    permission."""

    from AppKit import NSApplicationActivateIgnoringOtherApps

    running = _find_running(app)
    if running is None:
        raise LookupError(f"screen: no running app matches {app!r}; launch() it first.")
    running.activateWithOptions_(NSApplicationActivateIgnoringOtherApps)


def terminate(app: str) -> None:
    """Ask a running application to quit, by name or bundle id (a normal quit, so
    it may prompt to save). Raises `LookupError` if it is not running. Needs no
    Accessibility permission."""

    running = _find_running(app)
    if running is None:
        raise LookupError(f"screen: no running app matches {app!r}.")
    running.terminate()
