"""Adverse system events as agent notifications (macOS + Linux, default on).

The host degrading -- a disk filling up, the kernel under memory pressure, a
thermally throttled CPU, the OOM killer firing, a runaway load average -- is
exactly the context an agent needs and never thinks to poll for. This source
checks a handful of cheap interfaces once a minute and surfaces each condition
as one channel event when it BEGINS (edge-triggered: a disk that stays full is
one event, not one per minute). ``IX_MCP_NOTIFY_SYSTEM=0`` turns it off.

Probes, all O(one file read or one short subprocess) per tick:

  - low disk space on ``/`` and the home filesystem (``os.statvfs``);
  - high 1-minute load average relative to the CPU count (``os.getloadavg``);
  - macOS: memory pressure via ``sysctl kern.memorystatus_vm_pressure_level``
    and thermal throttling via ``pmset -g therm`` -- deliberately NOT
    ``log stream``, which is far too expensive to leave running;
  - Linux: memory pressure via PSI (``/proc/pressure/memory``, absent on old
    kernels -- degrades silently) and OOM kills via the ``oom_kill`` counter
    in ``/proc/vmstat`` (no journal/kmsg access needed).

Each probe is a plain callable returning the currently-active conditions
(``{key: Event}``); a probe that raises repeatedly is disabled alone with one
stderr line, and only when every probe is dead does the source disable itself.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import ClassVar

from . import Event, Source, SourceUnavailable

# One probe: the currently-active adverse conditions, keyed by a stable
# identifier. The source edge-triggers on keys, so a key must stay identical
# while its condition persists and change (or disappear and return) to re-fire.
Probe = Callable[[], dict[str, Event]]

_PROBE_MAX_FAILURES = 3

_DISK_MIN_FREE_FRACTION = 0.05
_DISK_MIN_FREE_BYTES = 2 * 1024**3
_LOAD_PER_CPU = 4.0
_PSI_SOME_AVG60 = 20.0

_SUBPROCESS_TIMEOUT = 5.0


def _run(argv: list[str]) -> str | None:
    """stdout of a short probe command, or None on any failure (probes degrade,
    never raise, when a host lacks the tool or answers slowly)."""
    try:
        proc = subprocess.run(
            argv, capture_output=True, text=True, timeout=_SUBPROCESS_TIMEOUT, check=False
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    return proc.stdout


def _disk_probe(paths: Sequence[Path] = (Path("/"),)) -> dict[str, Event]:
    """Filesystems under the free-space floor, deduplicated by device."""
    found: dict[str, Event] = {}
    seen: set[int] = set()
    for path in paths:
        try:
            device = path.stat().st_dev
            usage = os.statvfs(path)
        except OSError:
            continue
        if device in seen:
            continue
        seen.add(device)
        free = usage.f_bavail * usage.f_frsize
        total = usage.f_blocks * usage.f_frsize
        if total <= 0:
            continue
        fraction = free / total
        if free < _DISK_MIN_FREE_BYTES or fraction < _DISK_MIN_FREE_FRACTION:
            found[f"disk:{path}"] = Event(
                content=(
                    f"low disk space on {path}: {free / 1024**3:.1f} GiB free "
                    f"({fraction:.0%} of {total / 1024**3:.0f} GiB)"
                ),
                meta={"kind": "disk", "path": str(path)},
            )
    return found


def _default_disk_probe() -> dict[str, Event]:
    return _disk_probe((Path("/"), Path.home()))


def _load_probe() -> dict[str, Event]:
    """A 1-minute load average far past the CPU count."""
    try:
        load1 = os.getloadavg()[0]
    except OSError:
        return {}
    cpus = os.cpu_count() or 1
    if load1 < _LOAD_PER_CPU * cpus:
        return {}
    return {
        "load": Event(
            content=f"high system load: 1-minute load average {load1:.1f} on {cpus} CPUs",
            meta={"kind": "load"},
        )
    }


# kern.memorystatus_vm_pressure_level: 1 normal, 2 warning, 4 critical. The
# level is part of the key so an escalation (warning -> critical) re-fires.
_MACOS_PRESSURE_LEVELS = {2: "warning", 4: "critical"}


def _macos_memory_probe() -> dict[str, Event]:
    out = _run(["sysctl", "-n", "kern.memorystatus_vm_pressure_level"])
    if out is None:
        return {}
    try:
        level = int(out.strip())
    except ValueError:
        return {}
    label = _MACOS_PRESSURE_LEVELS.get(level)
    if label is None:
        return {}
    return {
        f"memory-pressure:{label}": Event(
            content=f"memory pressure is {label} (kern.memorystatus_vm_pressure_level={level})",
            meta={"kind": "memory", "level": label},
        )
    }


_THERM_LIMIT = re.compile(r"CPU_Speed_Limit\s*=\s*(\d+)")


def _macos_thermal_probe() -> dict[str, Event]:
    out = _run(["pmset", "-g", "therm"])
    if out is None:
        return {}
    match = _THERM_LIMIT.search(out)
    if match is None:
        return {}
    limit = int(match.group(1))
    if limit >= 100:
        return {}
    return {
        "thermal": Event(
            content=f"thermal throttling: CPU speed limited to {limit}%",
            meta={"kind": "thermal"},
        )
    }


_PSI_MEMORY = Path("/proc/pressure/memory")
_PSI_AVG60 = re.compile(r"^some .*\bavg60=([0-9.]+)", re.MULTILINE)


def _psi_memory_probe() -> dict[str, Event]:
    """Linux PSI: the share of the last minute some task stalled on memory."""
    try:
        text = _PSI_MEMORY.read_text()
    except OSError:
        return {}  # pre-4.20 kernel or PSI off: degrade silently
    match = _PSI_AVG60.search(text)
    if match is None:
        return {}
    avg60 = float(match.group(1))
    if avg60 < _PSI_SOME_AVG60:
        return {}
    return {
        "memory-pressure": Event(
            content=(
                f"memory pressure: tasks stalled on memory {avg60:.0f}% of the "
                "last minute (PSI some avg60)"
            ),
            meta={"kind": "memory"},
        )
    }


def _oom_probe() -> Probe:
    """Linux OOM kills, from the ``oom_kill`` counter in ``/proc/vmstat``.

    Stateful (a closure): the first read baselines, and each increase fires an
    event whose key embeds the new counter value, so consecutive kill batches
    each notify while a quiet counter re-arms nothing.
    """
    last: int | None = None

    def probe() -> dict[str, Event]:
        nonlocal last
        try:
            text = Path("/proc/vmstat").read_text()
        except OSError:
            return {}
        value: int | None = None
        for line in text.splitlines():
            if line.startswith("oom_kill "):
                value = int(line.split()[1])
                break
        if value is None:
            return {}
        previous, last = last, value
        if previous is None or value <= previous:
            return {}
        return {
            f"oom:{value}": Event(
                content=f"OOM killer fired: {value - previous} process(es) killed since the last check",
                meta={"kind": "oom", "killed": str(value - previous)},
            )
        }

    probe.__name__ = "_oom_probe"
    return probe


def _default_probes() -> tuple[Probe, ...]:
    probes: list[Probe] = [_default_disk_probe, _load_probe]
    if sys.platform == "darwin":
        probes += [_macos_memory_probe, _macos_thermal_probe]
    elif sys.platform.startswith("linux"):
        probes += [_psi_memory_probe, _oom_probe()]
    return tuple(probes)


class SystemEventsSource(Source):
    """Adverse host conditions, one event when each begins (edge-triggered)."""

    name: ClassVar[str] = "system"
    platforms: ClassVar[tuple[str, ...] | None] = ("darwin", "linux")
    interval: ClassVar[float] = 60.0

    def __init__(self, probes: Sequence[Probe] | None = None) -> None:
        self._probes: tuple[Probe, ...] = (
            tuple(probes) if probes is not None else _default_probes()
        )
        self._failures: dict[int, int] = {}
        self._active: set[str] = set()

    def available(self) -> str | None:
        if not self._probes:
            return "no probes for this platform"
        return None

    def poll(self) -> list[Event]:
        current: dict[str, Event] = {}
        for index, probe in enumerate(self._probes):
            if self._failures.get(index, 0) >= _PROBE_MAX_FAILURES:
                continue
            try:
                current.update(probe())
            except Exception as exc:
                # One broken probe never takes down its siblings: it alone is
                # retried, then disabled with one line.
                self._failures[index] = self._failures.get(index, 0) + 1
                if self._failures[index] >= _PROBE_MAX_FAILURES:
                    name = getattr(probe, "__name__", "probe")
                    print(
                        f"[ix-mcp] notifications: system probe {name} disabled "
                        f"after {_PROBE_MAX_FAILURES} consecutive failures ({exc!r})",
                        file=sys.stderr,
                        flush=True,
                    )
                continue
            self._failures[index] = 0
        if all(
            self._failures.get(index, 0) >= _PROBE_MAX_FAILURES
            for index in range(len(self._probes))
        ):
            raise SourceUnavailable("every system probe failed repeatedly")
        # Edge-trigger: only conditions that just appeared notify; a condition
        # that persists stays silent, and one that clears re-arms its key.
        fresh = [key for key in current if key not in self._active]
        self._active = set(current)
        return [current[key] for key in fresh]
