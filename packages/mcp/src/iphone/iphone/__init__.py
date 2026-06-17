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
- Coordinate input (``tap``/``swipe``/``type_text``/``press``/``unlock``) goes
  through WebDriverAgent and needs WDA installed/launchable on the device; the
  calls raise a clear error if it is not.

A single device is assumed when ``udid`` is omitted; pass ``udid`` when more than
one is attached (``devices()`` lists them). Targeting is done through the CLI's
``PYMOBILEDEVICE3_UDID``/``PYMOBILEDEVICE3_TUNNEL`` env vars.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import shutil
import sys
import tempfile
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
    "ensure_developer_ready",
    "info",
    "kill",
    "launch",
    "press",
    "screenshot",
    "simulate_location",
    "start_tunneld",
    "stop_tunneld",
    "swipe",
    "tap",
    "tunnels",
    "type_text",
    "unlock",
    "wda_status",
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
    ``start_tunneld`` and ``ensure_developer_ready``).
    """
    from PIL import Image

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
                return f"tunneld started (pid {_tunneld_proc.pid})"
        raise IphoneError("tunneld did not become ready within 20s")
    finally:
        # The daemon keeps its own open fd (POSIX), so removing the path now is
        # safe and avoids leaking a log file per start.
        await asyncio.to_thread(_unlink, err_log.name)


async def stop_tunneld() -> None:
    """Stop the tunneld daemon started by this module (no-op if not running)."""
    global _tunneld_proc

    if _tunneld_proc is None:
        return
    if _tunneld_proc.returncode is None:
        _tunneld_proc.terminate()
        try:
            await asyncio.wait_for(_tunneld_proc.wait(), timeout=10)
        except TimeoutError:
            _tunneld_proc.kill()
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


async def wda_status(udid: str | None = None) -> dict[str, object]:
    """WebDriverAgent status (requires WDA installed/launchable on the device)."""
    target = await _resolve(udid)
    data = await _run_json(["developer", "wda", "status"], udid=target, timeout=60)
    return data if isinstance(data, dict) else {"value": data}


async def tap(selector: str, udid: str | None = None) -> None:
    """Tap an element via WebDriverAgent (e.g. its visible label)."""
    target = await _resolve(udid)
    # `--` so a selector starting with `-` is not parsed as a CLI option.
    await _run(["developer", "wda", "tap", "--", selector], udid=target, timeout=60)


async def swipe(
    start_x: int,
    start_y: int,
    end_x: int,
    end_y: int,
    udid: str | None = None,
) -> None:
    """Swipe between two screen coordinates via WebDriverAgent."""
    target = await _resolve(udid)
    await _run(
        ["developer", "wda", "swipe", "--", str(start_x), str(start_y), str(end_x), str(end_y)],
        udid=target,
        timeout=60,
    )


async def type_text(text: str, udid: str | None = None) -> None:
    """Type text into the focused element via WebDriverAgent."""
    target = await _resolve(udid)
    # `--` so text starting with `-` is not parsed as a CLI option.
    await _run(["developer", "wda", "type", "--", text], udid=target, timeout=60)


async def press(button: str, udid: str | None = None) -> None:
    """Press a hardware button via WebDriverAgent (home/lock/volumeup/volumedown)."""
    target = await _resolve(udid)
    await _run(["developer", "wda", "press", "--", button], udid=target, timeout=60)


async def unlock(udid: str | None = None) -> None:
    """Unlock the device via WebDriverAgent."""
    target = await _resolve(udid)
    await _run(["developer", "wda", "unlock"], udid=target, timeout=60)
