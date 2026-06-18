"""Drive a USB-connected iPhone/iPad from the ix-mcp interpreter.

Bundled like `screen`/`vmkit`/`imessage` so any session can `import iphone` with
no install step. It is a thin async wrapper over the vendored ``pymobiledevice3``
CLI (pinned 9.27.0 for iOS 26 support): device data comes back as polars frames
and screenshots as PIL images, so returning them from a cell renders inline.

    import iphone
    await iphone.devices()                 # connected devices, one row each
    await iphone.start_tunneld(sudo=True)   # root tunnel daemon (iOS 17+)
    await iphone.ensure_developer_ready()   # mount the Developer Disk Image
    img = await iphone.screenshot()         # -> PIL.Image, renders inline
    await iphone.apps()                     # installed apps as polars
    await iphone.launch("com.apple.Preferences")

What needs what (iOS 17+):

- ``devices`` / ``info`` / ``apps`` work over plain USB (lockdown), no tunnel.
- The *developer* commands (``screenshot``, ``launch``, ``ensure_developer_ready``,
  ``simulate_location``, the ``wda`` family) need a running root ``tunneld`` AND
  the device in Developer Mode with the Developer Disk Image mounted. Start the
  daemon explicitly with ``start_tunneld(sudo=True)`` — no other call ever runs
  sudo. Enabling Developer Mode itself is a one-time on-device step (Settings >
  Privacy & Security > Developer Mode, then reboot + confirm); it cannot be done
  remotely, so ``ensure_developer_ready`` raises a clear error when it is off.
- UI input (``tap(x, y)`` / ``tap_element`` / ``swipe`` / ``type_text`` /
  ``press`` / ``home`` / ``unlock``) goes through WebDriverAgent over the W3C
  Actions API. WDA must be built/installed once (``wda_build_install``, needs
  full Xcode + an Apple ID in Xcode) and started (``wda_start(sudo=True)``);
  ``source()`` returns the live accessibility tree (with rects) as a polars
  frame, which is ground truth for where to tap even when a GPU/Metal view
  screenshots black.

Run ``iphone.doctor()`` to see exactly which prerequisite is missing.

    await iphone.wda_start(sudo=True)            # tunnel + WDA runner + forward
    df = await iphone.source()                    # elements with x/y/width/height
    row = df.filter(pl.col("name") == "some_id").row(0, named=True)
    await iphone.tap(row["x"] + row["width"]//2, row["y"] + row["height"]//2)
    await iphone.type_text("hello")

A single device is assumed when ``udid`` is omitted; pass ``udid`` when more than
one is attached (``devices()`` lists them). Targeting is done through the CLI's
``PYMOBILEDEVICE3_UDID``/``PYMOBILEDEVICE3_TUNNEL`` env vars.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
import re
import shutil
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import polars as pl
    from PIL import Image

__all__ = [
    "IphoneError",
    "apps",
    "clear_location",
    "developer_mode_status",
    "devices",
    "doctor",
    "ensure_developer_ready",
    "home",
    "info",
    "kill",
    "launch",
    "press",
    "screenshot",
    "simulate_location",
    "source",
    "start_tunneld",
    "stop_tunneld",
    "swipe",
    "tap",
    "tap_element",
    "tunnels",
    "type_text",
    "unlock",
    "wda_build_install",
    "wda_installed",
    "wda_start",
    "wda_status",
    "wda_stop",
]

_TUNNELD_HOST = "127.0.0.1"
_TUNNELD_PORT = 49151

# The detached root daemon start_tunneld owns, so stop_tunneld can end it.
_tunneld_proc: asyncio.subprocess.Process | None = None


class IphoneError(RuntimeError):
    """A pymobiledevice3 invocation failed, or a precondition was not met."""


def _pmd3_argv() -> list[str]:
    """Resolve the pymobiledevice3 executable as an argv prefix.

    Prefers the console script sitting next to the running interpreter (where the
    bundled interpreter installs it), then ``$PATH``, then ``python -m
    pymobiledevice3`` so the module is usable from a source tree too.
    """
    sibling = Path(sys.executable).parent / "pymobiledevice3"
    if sibling.exists():
        return [str(sibling)]
    found = shutil.which("pymobiledevice3")
    if found is not None:
        return [found]
    return [sys.executable, "-m", "pymobiledevice3"]


def _env(udid: str | None) -> dict[str, str]:
    """Subprocess environment targeting a specific device, if given."""
    env = dict(os.environ)
    if udid is not None:
        # PYMOBILEDEVICE3_UDID targets lockdown commands; PYMOBILEDEVICE3_TUNNEL
        # routes developer commands through tunneld for that device.
        env["PYMOBILEDEVICE3_UDID"] = udid
        env["PYMOBILEDEVICE3_TUNNEL"] = udid
    return env


async def _run(
    args: list[str],
    *,
    udid: str | None = None,
    timeout: float = 120.0,
    sudo: bool = False,
) -> str:
    """Run a pymobiledevice3 subcommand, returning stdout (raising on failure)."""
    argv = _pmd3_argv() + args
    if sudo:
        # -n: never prompt. Passwordless sudo must be configured; otherwise this
        # fails fast rather than hanging on a TTY prompt.
        argv = ["sudo", "-n", *argv]
    proc = await asyncio.create_subprocess_exec(
        *argv,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=_env(udid),
    )
    try:
        out, err = await asyncio.wait_for(proc.communicate(), timeout=timeout)
    except TimeoutError as exc:
        proc.kill()
        raise IphoneError(f"`{' '.join(args)}` timed out after {timeout:.0f}s") from exc
    if proc.returncode != 0:
        detail = (err or b"").decode(errors="replace").strip()
        raise IphoneError(f"`{' '.join(args)}` failed (exit {proc.returncode}): {detail}")
    return (out or b"").decode(errors="replace")


async def _run_json(
    args: list[str],
    *,
    udid: str | None = None,
    timeout: float = 120.0,
) -> object:
    """Run a subcommand whose stdout is JSON and parse it."""
    raw = await _run(args, udid=udid, timeout=timeout)
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise IphoneError(f"`{' '.join(args)}` did not return JSON: {raw[:200]}") from exc


async def _one_device() -> str:
    """The UDID of the sole connected device, or an error if not exactly one."""
    frame = await devices()
    raw = frame["UniqueDeviceID"].to_list() if "UniqueDeviceID" in frame.columns else []
    # One physical device can appear twice (USB + Wi-Fi) with the same UDID, so
    # dedupe before deciding whether targeting is ambiguous.
    udids = list(dict.fromkeys(str(u) for u in raw))
    if not udids:
        raise IphoneError("no device connected (plug in an iPhone/iPad over USB and trust it)")
    if len(udids) > 1:
        raise IphoneError(f"{len(udids)} devices connected; pass udid= (see iphone.devices())")
    return udids[0]


async def _resolve(udid: str | None) -> str:
    """Return the given UDID, or auto-select the single connected device."""
    return udid if udid is not None else await _one_device()


async def devices() -> pl.DataFrame:
    """Connected devices over usbmux, one row each.

    Columns are the raw lockdown keys: UniqueDeviceID, DeviceName, ProductType,
    ProductVersion, ConnectionType, ...
    """
    import polars as pl

    data = await _run_json(["usbmux", "list"], timeout=30)
    rows = data if isinstance(data, list) else []
    return pl.DataFrame(rows)


async def info(udid: str | None = None) -> dict[str, object]:
    """Full lockdown device-info dictionary for the (selected) device."""
    target = await _resolve(udid)
    data = await _run_json(["lockdown", "info"], udid=target, timeout=30)
    return data if isinstance(data, dict) else {"value": data}


async def apps(udid: str | None = None) -> pl.DataFrame:
    """Installed apps as a polars frame (bundle_id, name, version, type)."""
    import polars as pl

    target = await _resolve(udid)
    data = await _run_json(["apps", "list"], udid=target, timeout=120)
    items = data.items() if isinstance(data, dict) else []
    rows = [
        {
            "bundle_id": bundle_id,
            "name": meta.get("CFBundleDisplayName") if isinstance(meta, dict) else None,
            "version": meta.get("CFBundleShortVersionString") if isinstance(meta, dict) else None,
            "type": meta.get("ApplicationType") if isinstance(meta, dict) else None,
        }
        for bundle_id, meta in items
    ]
    return pl.DataFrame(rows)


async def screenshot(udid: str | None = None) -> Image.Image:
    """Capture the device screen as a PIL image (renders inline from a cell).

    Needs a running tunnel + a mounted Developer Disk Image (see
    ``start_tunneld`` and ``ensure_developer_ready``). When WebDriverAgent is
    running, captures via WDA instead: while an XCUITest session holds the
    display, the DVT screenshot returns a black frame, so WDA is the reliable
    source during automation.
    """
    import base64
    import io

    from PIL import Image

    # Use the WDA capture only when it targets the requested device (WDA serves
    # the one device wda_start bound); otherwise fall through to DVT for `udid`.
    if (udid is None or udid == _wda_device) and await _wda_up():
        value = await _wda("GET", "/screenshot")
        png = base64.b64decode(str(value))
        with Image.open(io.BytesIO(png)) as img:
            return img.copy()

    target = await _resolve(udid)
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "screenshot.png"
        await _run(["developer", "dvt", "screenshot", str(out)], udid=target, timeout=120)
        if not out.exists():
            raise IphoneError("screenshot produced no file (is the developer tunnel up?)")
        with Image.open(out) as img:
            return img.copy()


async def launch(bundle_id: str, udid: str | None = None) -> int:
    """Launch an app by bundle id; returns its new process id (-1 if unknown)."""
    target = await _resolve(udid)
    out = await _run(["developer", "dvt", "launch", bundle_id], udid=target, timeout=60)
    match = re.search(r"pid\s+(\d+)", out)
    return int(match.group(1)) if match else -1


async def kill(pid: int, udid: str | None = None) -> None:
    """Kill a process on the device by pid."""
    target = await _resolve(udid)
    await _run(["developer", "dvt", "kill", str(pid)], udid=target, timeout=30)


async def developer_mode_status(udid: str | None = None) -> bool:
    """Whether Developer Mode is enabled on the device."""
    target = await _resolve(udid)
    out = await _run(["amfi", "developer-mode-status"], udid=target, timeout=30)
    return "true" in out.lower()


async def ensure_developer_ready(udid: str | None = None) -> None:
    """Mount the Developer Disk Image, after verifying Developer Mode is on.

    Raises a clear, actionable error if Developer Mode is off — enabling it is a
    one-time on-device step (reboot + physical confirm) that cannot be automated.
    """
    target = await _resolve(udid)
    if not await developer_mode_status(target):
        raise IphoneError(
            "Developer Mode is off. Enable it on the device: Settings > Privacy & "
            "Security > Developer Mode, toggle on, then reboot and confirm with your "
            "passcode. Apple requires this be done physically; it cannot be done remotely."
        )
    await _run(["mounter", "auto-mount"], udid=target, timeout=180)


def _read_text(path: str) -> str:
    """Read a small text file (run off-thread from async callers)."""
    return Path(path).read_text(errors="replace")


def _unlink(path: str) -> None:
    """Remove a file, ignoring a missing one (run off-thread from async callers)."""
    Path(path).unlink(missing_ok=True)


def _exists(path: str) -> bool:
    """Whether a path exists (sync helper so async callers avoid pathlib lint)."""
    return Path(path).exists()


def _derived_data_dir() -> str:
    """Xcode's DerivedData directory (sync helper to keep pathlib out of async)."""
    return str(Path.home() / "Library/Developer/Xcode/DerivedData")


def _newest(paths: list[str]) -> str:
    """The most recently modified path (sync helper; keeps pathlib out of async)."""
    return max(paths, key=lambda p: Path(p).stat().st_mtime)


def _default_wda_dir() -> str:
    """A user-owned WebDriverAgent checkout dir (not world-writable /tmp).

    Cloning/building from a predictable, world-writable path would let another
    user plant code we then sign and install; keep it under the user's cache.
    """
    base = Path.home() / ".cache" / "ix-iphone"
    base.mkdir(parents=True, exist_ok=True)
    return str(base / "WebDriverAgent")


async def _group_signal(pid: int, signal: str) -> None:
    """Send `signal` to a sudo-spawned process group (negative pid).

    Raises IphoneError if the `sudo kill` itself fails (e.g. sudoers permits
    starting tunneld but not killing it), so cleanup never reports false success.
    """
    killer = await asyncio.create_subprocess_exec(
        "sudo",
        "-n",
        "kill",
        signal,
        f"-{pid}",
        stdout=asyncio.subprocess.DEVNULL,
        stderr=asyncio.subprocess.PIPE,
    )
    _, err = await killer.communicate()
    if killer.returncode != 0:
        detail = (err or b"").decode(errors="replace").strip()
        raise IphoneError(
            f"`sudo kill {signal} -{pid}` failed (exit {killer.returncode}): "
            f"{detail or 'no output'}"
        )


async def _sudo_kill(proc: asyncio.subprocess.Process | None) -> None:
    """Terminate a sudo-spawned (root) tunneld process group; no-op if gone.

    The daemon runs as root, so the spawning user cannot signal it directly
    (`proc.terminate()` would EPERM); route the kill through sudo. The process
    was started with its own session (`start_new_session=True`), so its pid is
    the process-group id and a negative-pid kill reaps sudo and tunneld together.
    Escalates to SIGKILL if the group ignores SIGTERM. Fails closed: raises
    IphoneError if the signal could not be sent or the process is still alive
    afterward, so a caller never clears its handle on a privileged process that
    is in fact still running.
    """
    if proc is None or proc.returncode is not None:
        return
    await _group_signal(proc.pid, "-TERM")
    with contextlib.suppress(TimeoutError):
        await asyncio.wait_for(proc.wait(), timeout=5)
        return
    await _group_signal(proc.pid, "-KILL")
    try:
        await asyncio.wait_for(proc.wait(), timeout=5)
    except TimeoutError:
        raise IphoneError(
            f"tunneld (pid {proc.pid}) still running after SIGKILL; left tracked for retry"
        ) from None


def _tunneld_url() -> str:
    return f"http://{_TUNNELD_HOST}:{_TUNNELD_PORT}/"


def _fetch_tunnels() -> dict[str, object]:
    """Query the tunneld HTTP API; empty dict if it is not running."""
    try:
        with urllib.request.urlopen(_tunneld_url(), timeout=2) as resp:  # noqa: S310 -- fixed localhost URL
            parsed = json.loads(resp.read())
            return parsed if isinstance(parsed, dict) else {}
    except (OSError, json.JSONDecodeError):
        return {}


async def _tunneld_up() -> bool:
    try:
        await asyncio.to_thread(
            lambda: urllib.request.urlopen(_tunneld_url(), timeout=2).close()  # noqa: S310 -- fixed localhost URL
        )
    except OSError:
        return False
    return True


async def tunnels() -> pl.DataFrame:
    """Active tunnels reported by tunneld, one row per device tunnel."""
    import polars as pl

    data = await asyncio.to_thread(_fetch_tunnels)
    rows: list[dict[str, object]] = [
        {
            "udid": udid,
            "tunnel_address": entry.get("tunnel-address"),
            "tunnel_port": entry.get("tunnel-port"),
            "interface": entry.get("interface"),
        }
        for udid, entries in data.items()
        for entry in (entries if isinstance(entries, list) else [])
        if isinstance(entry, dict)
    ]
    return pl.DataFrame(rows)


async def start_tunneld(*, sudo: bool = False) -> str:
    """Start the persistent root ``tunneld`` daemon required by iOS 17+ services.

    This launches ``sudo pymobiledevice3 remote tunneld`` as a detached root
    process that outlives the cell. Because it runs as root, it must be opted into
    explicitly: calling without ``sudo=True`` raises rather than escalating. No
    other function in this module ever invokes sudo.
    """
    global _tunneld_proc

    if not sudo:
        raise IphoneError(
            "start_tunneld launches a ROOT daemon (`sudo pymobiledevice3 remote "
            "tunneld`). Re-run as iphone.start_tunneld(sudo=True) to allow it "
            "(passwordless sudo must be configured)."
        )
    if await _tunneld_up():
        return "tunneld already running"

    # Send the daemon's stderr to a file rather than a pipe: a healthy tunneld
    # logs continuously and would eventually block on a full, undrained pipe,
    # while a file lets us recover the message if it dies early (e.g. `sudo -n`
    # failing on a host without passwordless sudo).
    err_log = tempfile.NamedTemporaryFile(  # noqa: SIM115 -- handed to the child; closed below
        prefix="ix-tunneld-", suffix=".log", delete=False
    )
    argv = ["sudo", "-n", *_pmd3_argv(), "remote", "tunneld"]
    _tunneld_proc = await asyncio.create_subprocess_exec(
        *argv,
        stdout=asyncio.subprocess.DEVNULL,
        stderr=err_log,
        start_new_session=True,
    )
    err_log.close()
    started = False
    try:
        for _ in range(40):
            await asyncio.sleep(0.5)
            if _tunneld_proc.returncode is not None:
                detail = (await asyncio.to_thread(_read_text, err_log.name)).strip()
                raise IphoneError(
                    f"tunneld exited ({_tunneld_proc.returncode}) before becoming ready: "
                    f"{detail or 'no output (check passwordless sudo)'}"
                )
            if await _tunneld_up():
                started = True
                return f"tunneld started (pid {_tunneld_proc.pid})"
        raise IphoneError("tunneld did not become ready within 20s")
    finally:
        # The daemon keeps its own open fd (POSIX), so removing the path now is
        # safe and avoids leaking a log file per start.
        await asyncio.to_thread(_unlink, err_log.name)
        # On any non-success exit (early exit or readiness timeout) do not leave a
        # privileged tunneld running behind a "startup failed" error. Suppress a
        # cleanup failure so it cannot mask the original startup error, and only
        # drop the handle if the daemon actually died (else keep it so a later
        # stop_tunneld() can retry the kill).
        if not started:
            with contextlib.suppress(IphoneError):
                await _sudo_kill(_tunneld_proc)
            if _tunneld_proc is not None and _tunneld_proc.returncode is not None:
                _tunneld_proc = None


async def stop_tunneld() -> None:
    """Stop the tunneld daemon started by this module (no-op if not running)."""
    global _tunneld_proc

    if _tunneld_proc is None:
        return
    await _sudo_kill(_tunneld_proc)
    _tunneld_proc = None


async def simulate_location(latitude: float, longitude: float, udid: str | None = None) -> None:
    """Override the device's GPS location (iOS 17+, via DVT)."""
    target = await _resolve(udid)
    await _run(
        ["developer", "dvt", "simulate-location", "set", "--", str(latitude), str(longitude)],
        udid=target,
        timeout=30,
    )


async def clear_location(udid: str | None = None) -> None:
    """Stop simulating GPS and restore the device's real location."""
    target = await _resolve(udid)
    await _run(["developer", "dvt", "simulate-location", "clear"], udid=target, timeout=30)


# ---------------------------------------------------------------------------
# WebDriverAgent UI control
#
# iOS exposes no developer service that synthesizes a touch, so taps/typing go
# through Apple's XCTest framework, which WebDriverAgent wraps as an HTTP
# (WebDriver) server. We drive that HTTP directly over a usbmux port-forward and
# use the W3C Actions API: pymobiledevice3's own `developer wda` subcommands are
# version-fragile (its tap endpoint 404s on WDA 14.x and its by-name lookups go
# stale), whereas raw W3C actions are stable.
# ---------------------------------------------------------------------------

_WDA_BUNDLE = "com.facebook.WebDriverAgentRunner.xctrunner"
_WDA_LOCAL_PORT = 8100  # local end of the usbmux forward to WDA's device port

# Long-lived processes wda_start owns: the XCUITest runner (launches WDA) and the
# usbmux TCP forward to it. Plus the cached WebDriver session id.
_wda_xcuitest_proc: asyncio.subprocess.Process | None = None
_wda_forward_proc: asyncio.subprocess.Process | None = None
_wda_session: str | None = None
# The device the running WDA runner/forward target, so wda_start does not silently
# report "already running" for a different udid than the caller asked for.
_wda_device: str | None = None


def _wda_call(method: str, path: str, body: dict[str, object] | None) -> object:
    """One blocking WDA HTTP call (run off-thread). Returns the parsed `value`."""
    url = f"http://{_TUNNELD_HOST}:{_WDA_LOCAL_PORT}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)  # noqa: S310 -- fixed localhost
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:  # noqa: S310 -- fixed localhost
            raw = resp.read()
    except urllib.error.HTTPError as exc:
        raise IphoneError(f"WDA {method} {path} -> HTTP {exc.code}: {exc.read()[:200]!r}") from exc
    except OSError as exc:
        raise IphoneError(
            f"WDA {method} {path} failed: {exc}. Is WDA running? (iphone.wda_start(sudo=True))"
        ) from exc
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        # A non-JSON body (proxy/error page, empty) must not crash readiness checks.
        raise IphoneError(f"WDA {method} {path} returned non-JSON: {raw[:200]!r}") from exc
    return payload.get("value") if isinstance(payload, dict) else payload


async def _wda(method: str, path: str, body: dict[str, object] | None = None) -> object:
    """Async wrapper around a WDA HTTP call."""
    return await asyncio.to_thread(_wda_call, method, path, body)


async def _wda_up() -> bool:
    """Whether WDA's HTTP server answers on the forwarded port."""
    try:
        await _wda("GET", "/status")
    except IphoneError:
        return False
    return True


async def _wda_sid() -> str:
    """The cached WDA session id, creating one bound to the active app if needed."""
    global _wda_session
    if _wda_session:
        return _wda_session
    value = await _wda("POST", "/session", {"capabilities": {"alwaysMatch": {}}})
    if not isinstance(value, dict) or "sessionId" not in value:
        raise IphoneError(f"WDA session create returned no sessionId: {value!r}")
    _wda_session = str(value["sessionId"])
    return _wda_session


async def _wda_in_session(method: str, suffix: str, body: dict[str, object] | None = None) -> object:
    """Call a session-scoped WDA endpoint, healing a stale session once.

    WDA keeps answering /status after a session expires (app crash, idle timeout,
    runner restart), so a cached id can go dead while the server looks up. On a
    session error, drop the cached id, mint a fresh one, and retry once.
    """
    global _wda_session

    sid = await _wda_sid()
    try:
        return await _wda(method, f"/session/{sid}{suffix}", body)
    except IphoneError as exc:
        if "session" not in str(exc).lower():
            raise
        _wda_session = None
        sid = await _wda_sid()
        return await _wda(method, f"/session/{sid}{suffix}", body)


async def wda_status() -> dict[str, object]:
    """WebDriverAgent /status (raises if WDA is not reachable)."""
    value = await _wda("GET", "/status")
    return value if isinstance(value, dict) else {"value": value}


async def source() -> pl.DataFrame:
    """Accessibility tree of the foreground app as a polars frame.

    One row per element with name/label/value/type and on-screen rect (x, y, w, h
    in points). This is ground truth even when screenshots come back black (the
    map is a GPU surface): filter it to find a control, then `tap` its center.
    """
    import polars as pl

    tree = await _wda("GET", "/source?format=json")
    rows: list[dict[str, object]] = []

    def s(v: object) -> str | None:
        return None if v is None else str(v)

    def i(v: object) -> int | None:
        return int(v) if isinstance(v, (int, float)) else None

    def walk(node: object) -> None:
        if isinstance(node, dict):
            rect = node.get("rect")
            rect = rect if isinstance(rect, dict) else {}
            rows.append(
                {
                    "type": str(node.get("type", "")).replace("XCUIElementType", ""),
                    "name": s(node.get("name")),
                    "label": s(node.get("label")),
                    "value": s(node.get("value")),
                    "x": i(rect.get("x")),
                    "y": i(rect.get("y")),
                    "width": i(rect.get("width")),
                    "height": i(rect.get("height")),
                }
            )
            for child in node.get("children", []) or []:
                walk(child)

    walk(tree)
    # Explicit schema: text fields are strings, rects ints — the tree mixes
    # numeric-looking and text values per column, which trips dtype inference.
    schema = {
        "type": pl.String,
        "name": pl.String,
        "label": pl.String,
        "value": pl.String,
        "x": pl.Int64,
        "y": pl.Int64,
        "width": pl.Int64,
        "height": pl.Int64,
    }
    return pl.DataFrame(rows, schema=schema)


async def tap(x: int, y: int) -> None:
    """Tap at screen coordinates (points) on the active WDA device, via W3C actions."""
    await _wda_in_session(
        "POST",
        "/actions",
        {
            "actions": [
                {
                    "type": "pointer",
                    "id": "finger1",
                    "parameters": {"pointerType": "touch"},
                    "actions": [
                        {"type": "pointerMove", "duration": 0, "x": x, "y": y},
                        {"type": "pointerDown", "button": 0},
                        {"type": "pause", "duration": 90},
                        {"type": "pointerUp", "button": 0},
                    ],
                }
            ]
        },
    )


async def tap_element(*, name: str | None = None, label: str | None = None) -> None:
    """Find an element by accessibility name or label and tap its center."""
    frame = await source()
    import polars as pl

    expr = None
    if name is not None:
        expr = pl.col("name") == name
    if label is not None:
        lexpr = pl.col("label") == label
        expr = lexpr if expr is None else (expr | lexpr)
    if expr is None:
        raise IphoneError("tap_element needs name= or label=")
    match = frame.filter(expr).drop_nulls(["x", "y", "width", "height"])
    if match.height == 0:
        raise IphoneError(f"no element matched name={name!r} label={label!r}")
    row = match.row(0, named=True)
    await tap(int(row["x"] + row["width"] // 2), int(row["y"] + row["height"] // 2))


async def swipe(
    start_x: int,
    start_y: int,
    end_x: int,
    end_y: int,
    *,
    duration: float = 0.3,
) -> None:
    """Swipe between two screen coordinates (points) via the W3C Actions API."""
    await _wda_in_session(
        "POST",
        "/actions",
        {
            "actions": [
                {
                    "type": "pointer",
                    "id": "finger1",
                    "parameters": {"pointerType": "touch"},
                    "actions": [
                        {"type": "pointerMove", "duration": 0, "x": start_x, "y": start_y},
                        {"type": "pointerDown", "button": 0},
                        {"type": "pointerMove", "duration": int(duration * 1000), "x": end_x, "y": end_y},
                        {"type": "pointerUp", "button": 0},
                    ],
                }
            ]
        },
    )


async def type_text(text: str) -> None:
    """Type into the focused field (tap a field first) via WDA."""
    await _wda_in_session("POST", "/wda/keys", {"value": list(text)})


async def press(button: str) -> None:
    """Press a hardware button via WDA (home / volumeUp / volumeDown)."""
    await _wda_in_session("POST", "/wda/pressButton", {"name": button})


async def home() -> None:
    """Go to the home screen via WDA (the home-button gesture)."""
    await _wda_in_session("POST", "/wda/pressButton", {"name": "home"})


async def unlock() -> None:
    """Wake and unlock the device via WDA."""
    await _wda("POST", "/wda/unlock")


# ---------------------------------------------------------------------------
# WDA lifecycle: install (one-time, needs Xcode) and start/stop (runtime)
# ---------------------------------------------------------------------------


async def _sh(
    argv: list[str],
    *,
    timeout: float = 600.0,
    env: dict[str, str] | None = None,
) -> tuple[int, str, str]:
    """Run an arbitrary command, returning (returncode, stdout, stderr)."""
    try:
        proc = await asyncio.create_subprocess_exec(
            *argv,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=env if env is not None else dict(os.environ),
        )
    except FileNotFoundError as exc:
        raise IphoneError(f"`{argv[0]}` not found on PATH") from exc
    try:
        out, err = await asyncio.wait_for(proc.communicate(), timeout=timeout)
    except TimeoutError as exc:
        proc.kill()
        raise IphoneError(f"`{argv[0]}` timed out after {timeout:.0f}s") from exc
    return proc.returncode or 0, out.decode(errors="replace"), err.decode(errors="replace")


async def wda_installed(udid: str | None = None) -> bool:
    """Whether the WebDriverAgent runner is installed on the device."""
    import polars as pl

    frame = await apps(udid)
    return frame.filter(pl.col("bundle_id") == _WDA_BUNDLE).height > 0


async def _detect_team() -> str | None:
    """Best-effort Apple Developer team id from the signing certificate's OU.

    Returns None (never raises) when the toolchain is absent, so callers like
    doctor() can report it as a missing prerequisite.
    """
    try:
        _, identities, _ = await _sh(["security", "find-identity", "-v", "-p", "codesigning"])
        cert = re.search(r'"(Apple Development:[^"]+)"', identities)
        if cert is None:
            return None
        rc, pem, _ = await _sh(["security", "find-certificate", "-c", cert.group(1), "-p"])
        if rc != 0 or not pem:
            return None
        with tempfile.NamedTemporaryFile("w", suffix=".pem", delete=False) as handle:
            handle.write(pem)
            pem_path = handle.name
        try:
            _, subject, _ = await _sh(["openssl", "x509", "-in", pem_path, "-noout", "-subject"])
        finally:
            await asyncio.to_thread(_unlink, pem_path)
    except IphoneError:
        return None
    team = re.search(r"OU\s*=\s*([A-Z0-9]+)", subject)
    return team.group(1) if team else None


async def wda_build_install(
    *,
    team: str | None = None,
    udid: str | None = None,
    wda_dir: str | None = None,
) -> str:
    """Build, sign, and install WebDriverAgent on the device (one-time).

    macOS + full Xcode + an Apple ID signed into Xcode (Settings > Accounts) are
    required; both are one-time human steps that cannot be automated. Uses
    build-for-testing to produce a signed runner, then installs it through the
    installation-proxy (CoreDevice's installer reports the capability as
    unsupported on current iOS, so the classic path is used instead).

    Full Xcode is selected per-invocation via ``DEVELOPER_DIR`` (no global
    ``xcode-select`` mutation), and the checkout defaults to a user-owned cache
    dir rather than a world-writable /tmp path.
    """
    if sys.platform != "darwin":
        raise IphoneError("WDA build needs macOS + Xcode")
    xcode = "/Applications/Xcode.app/Contents/Developer"
    if not _exists(xcode):
        raise IphoneError("Full Xcode not found at /Applications/Xcode.app (install it from the App Store)")
    wda_dir = wda_dir or _default_wda_dir()
    team = team or await _detect_team()
    if not team:
        raise IphoneError(
            "could not detect a signing team; sign your Apple ID into Xcode "
            "(Settings > Accounts) or pass team=..."
        )
    if not _exists(wda_dir):
        rc, _, err = await _sh(
            ["git", "clone", "--depth", "1", "https://github.com/appium/WebDriverAgent", wda_dir],
            timeout=240,
        )
        if rc != 0:
            raise IphoneError(f"cloning WebDriverAgent failed: {err[-200:]}")
    target = await _resolve(udid)
    env = dict(os.environ)
    env["DEVELOPER_DIR"] = xcode
    rc, out, err = await _sh(
        [
            "xcodebuild",
            "-project",
            f"{wda_dir}/WebDriverAgent.xcodeproj",
            "-scheme",
            "WebDriverAgentRunner",
            "-destination",
            f"id={target}",
            "-allowProvisioningUpdates",
            "-allowProvisioningDeviceRegistration",
            f"DEVELOPMENT_TEAM={team}",
            "build-for-testing",
        ],
        timeout=1200,
        env=env,
    )
    if rc != 0:
        raise IphoneError(f"xcodebuild failed (team {team}): {(err or out)[-400:]}")
    derived = _derived_data_dir()
    _, found, _ = await _sh(
        ["/usr/bin/find", derived, "-name", "WebDriverAgentRunner-Runner.app", "-type", "d"],
        timeout=30,
    )
    apps_found = [line for line in found.splitlines() if line.strip()]
    if not apps_found:
        raise IphoneError("built WebDriverAgentRunner-Runner.app not found in DerivedData")
    # DerivedData can hold builds from prior runs; install the freshest one.
    await _run(["apps", "install", _newest(apps_found)], udid=target, timeout=240)
    return f"WDA installed (team {team})"


async def wda_start(*, sudo: bool = False, udid: str | None = None) -> str:
    """Bring WebDriverAgent up: tunnel, launch the runner, forward its port.

    Idempotent. Needs WDA already installed (see ``wda_build_install``) and, for
    the developer tunnel, ``sudo=True`` (same root-daemon opt-in as
    ``start_tunneld``). Returns once WDA answers, with a session ready for taps.
    """
    global _wda_xcuitest_proc, _wda_forward_proc, _wda_session, _wda_device

    target = await _resolve(udid)
    if await _wda_up():
        if _wda_device == target or (_wda_device is None and udid is None):
            # Either this module started WDA for `target`, or there is one device
            # and no specific udid was requested (so it cannot be the wrong one).
            await _wda_sid()
            return "WDA already running"
        owner = _wda_device or "an unknown device (a forward this module did not start)"
        raise IphoneError(
            f"WDA is already reachable for {owner}; call iphone.wda_stop() before "
            f"starting it for {target}"
        )
    if not await _tunneld_up():
        await start_tunneld(sudo=sudo)
    if not await wda_installed(target):
        raise IphoneError(
            "WebDriverAgent is not installed. Run iphone.wda_build_install() "
            "(needs full Xcode + an Apple ID signed into Xcode)."
        )
    if _wda_xcuitest_proc is None or _wda_xcuitest_proc.returncode is not None:
        # A fresh runner means any cached session id is dead; force a new one.
        _wda_session = None
        _wda_device = target
        _wda_xcuitest_proc = await asyncio.create_subprocess_exec(
            *_pmd3_argv(),
            "developer",
            "dvt",
            "xcuitest",
            _WDA_BUNDLE,
            env=_env(target),
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
            start_new_session=True,
        )
    if _wda_forward_proc is None or _wda_forward_proc.returncode is not None:
        _wda_forward_proc = await asyncio.create_subprocess_exec(
            *_pmd3_argv(),
            "usbmux",
            "forward",
            str(_WDA_LOCAL_PORT),
            "8100",
            env=_env(target),
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
            start_new_session=True,
        )
    for _ in range(60):
        await asyncio.sleep(1)
        if await _wda_up():
            await _wda_sid()
            return f"WDA started (runner pid {_wda_xcuitest_proc.pid}, forward :{_WDA_LOCAL_PORT})"
    # Do not leave the runner/forward we just spawned behind a "not ready" error.
    await wda_stop()
    raise IphoneError("WDA did not become ready within 60s")


async def _terminate(proc: asyncio.subprocess.Process | None) -> None:
    """Terminate a process this module owns (non-root), escalating to kill."""
    if proc is None or proc.returncode is not None:
        return
    proc.terminate()
    try:
        await asyncio.wait_for(proc.wait(), timeout=5)
    except TimeoutError:
        proc.kill()


async def wda_stop() -> None:
    """Stop the WDA runner and port-forward started by this module."""
    global _wda_xcuitest_proc, _wda_forward_proc, _wda_session, _wda_device

    _wda_session = None
    _wda_device = None
    await _terminate(_wda_forward_proc)
    await _terminate(_wda_xcuitest_proc)
    _wda_forward_proc = None
    _wda_xcuitest_proc = None


async def doctor(udid: str | None = None) -> pl.DataFrame:
    """Report each prerequisite for full control as a polars frame of checks.

    Names exactly what is missing (device trust, Developer Mode, tunnel, WDA,
    Xcode signing) so setup is one glance instead of a multi-step debug. Never
    raises.
    """
    import polars as pl

    checks: list[dict[str, object]] = []

    def add(name: str, *, ok: bool, detail: str = "") -> None:
        checks.append({"check": name, "ok": ok, "detail": detail})

    target: str | None = udid
    try:
        frame = await devices()
        n = frame.height
        ids = frame["UniqueDeviceID"].to_list() if "UniqueDeviceID" in frame.columns else []
        connected = (udid in ids) if udid is not None else (n > 0)
        add("device connected", ok=connected, detail=udid if udid is not None else f"{n} listed")
        if target is None and ids:
            target = str(ids[0])
    except IphoneError as exc:
        add("device connected", ok=False, detail=str(exc)[:80])

    if target is not None:
        try:
            add("developer mode", ok=await developer_mode_status(target))
        except IphoneError as exc:
            add("developer mode", ok=False, detail=str(exc)[:80])

    add("tunneld running", ok=await _tunneld_up())
    add("WDA reachable", ok=await _wda_up())
    if target is not None:
        try:
            add("WDA installed", ok=await wda_installed(target))
        except IphoneError as exc:
            add("WDA installed", ok=False, detail=str(exc)[:80])

    if sys.platform == "darwin":
        add("full Xcode present", ok=_exists("/Applications/Xcode.app/Contents/Developer"))
        team = await _detect_team()
        add("signing team", ok=team is not None, detail=team or "no Apple Development cert / Xcode account")

    return pl.DataFrame(checks)
