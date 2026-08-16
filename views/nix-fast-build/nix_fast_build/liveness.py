import asyncio
import contextlib
import json
import os
import signal
import time
from asyncio.subprocess import Process
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

BIG_PARALLEL_FEATURE = "big-parallel"


class BuildStatusError(RuntimeError):
    def __init__(self, path: Path, source: Exception) -> None:
        self.path = path
        self.source = source
        super().__init__(path, source)

    def __str__(self) -> str:
        return f"cannot read Nix build status {self.path}: {self.source}"


@dataclass(frozen=True)
class LivenessPolicy:
    ordinary_seconds: float
    big_parallel_seconds: float
    poll_seconds: float = 1.0

    def deadline(self, required_system_features: frozenset[str]) -> float:
        if BIG_PARALLEL_FEATURE in required_system_features:
            return self.big_parallel_seconds
        return self.ordinary_seconds

    @property
    def enabled(self) -> bool:
        return self.ordinary_seconds > 0

    def nix_options(self) -> list[str]:
        if not self.enabled:
            return []
        return [
            "--option",
            "max-no-progress-time",
            str(int(self.ordinary_seconds)),
            "--option",
            "big-parallel-max-no-progress-time",
            str(int(self.big_parallel_seconds)),
        ]


@dataclass(frozen=True)
class ProgressSnapshot:
    phase: str
    goals: tuple[str, ...] = ()


@dataclass
class BuildWatch:
    attr: str
    drv_path: str
    deadline_seconds: float
    last_progress_at: float
    last_snapshot: ProgressSnapshot = field(
        default_factory=lambda: ProgressSnapshot(phase="waiting-for-builder")
    )

    def observe(self, snapshot: ProgressSnapshot, now: float) -> str | None:
        if snapshot.phase == "builder-active":
            # The daemon owns running-builder liveness and cancellation.
            self.last_snapshot = snapshot
            self.last_progress_at = now
            return None
        if snapshot != self.last_snapshot:
            self.last_snapshot = snapshot
            self.last_progress_at = now
            return None
        silent_for = now - self.last_progress_at
        if silent_for < self.deadline_seconds:
            return None
        return (
            f"derivation {self.drv_path} ({self.attr}) had no daemon-owned goal "
            f"for {silent_for:.0f}s; state={snapshot.phase}; goals={len(snapshot.goals)}"
        )


@dataclass
class _Subscription:
    watch: BuildWatch
    client_pid: int
    stalled: asyncio.Future[str]


class BuildLivenessMonitor:
    def __init__(
        self,
        policy: LivenessPolicy,
        *,
        state_dir: Path | None = None,
    ) -> None:
        self.policy = policy
        self.status_dir = (
            state_dir
            if state_dir is not None
            else Path(os.environ.get("NIX_STATE_DIR", "/nix/var/nix"))
        ) / "status"
        self._subscriptions: dict[int, _Subscription] = {}
        self._next_id = 0
        self._stop = asyncio.Event()

    async def run(self) -> None:
        while not self._stop.is_set():
            self.poll(time.monotonic())
            with contextlib.suppress(TimeoutError):
                await asyncio.wait_for(
                    self._stop.wait(), timeout=self.policy.poll_seconds
                )

    def stop(self) -> None:
        self._stop.set()

    def poll(self, now: float) -> None:
        entries = self._read_status_entries()
        for subscription in self._subscriptions.values():
            snapshot = _snapshot_for(subscription.client_pid, entries)
            diagnostic = subscription.watch.observe(snapshot, now)
            if diagnostic is not None and not subscription.stalled.done():
                subscription.stalled.set_result(diagnostic)

    async def wait_for_build(
        self,
        proc: Process,
        *,
        attr: str,
        drv_path: str,
        required_system_features: frozenset[str],
    ) -> tuple[int, str | None]:
        loop = asyncio.get_running_loop()
        subscription_id = self._next_id
        self._next_id += 1
        stalled: asyncio.Future[str] = loop.create_future()
        self._subscriptions[subscription_id] = _Subscription(
            watch=BuildWatch(
                attr=attr,
                drv_path=drv_path,
                deadline_seconds=self.policy.deadline(required_system_features),
                last_progress_at=time.monotonic(),
            ),
            client_pid=proc.pid,
            stalled=stalled,
        )
        process_done = asyncio.create_task(proc.wait())
        try:
            done, _ = await asyncio.wait(
                {
                    cast("asyncio.Future[object]", process_done),
                    cast("asyncio.Future[object]", stalled),
                },
                return_when=asyncio.FIRST_COMPLETED,
            )
            if process_done in done:
                return process_done.result(), None
            if proc.returncode is not None:
                return await process_done, None
            diagnostic = stalled.result()
            await _stop_process(proc)
            await process_done
            return 124, diagnostic
        finally:
            self._subscriptions.pop(subscription_id, None)
            if not process_done.done():
                process_done.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await process_done

    def _read_status_entries(self) -> list[dict[str, Any]]:
        if not self.status_dir.exists():
            return []
        entries: list[dict[str, Any]] = []
        for path in self.status_dir.glob("*.json"):
            try:
                payload = json.loads(path.read_text())
            except FileNotFoundError:
                continue
            except (OSError, json.JSONDecodeError) as error:
                raise BuildStatusError(path, error) from error
            if isinstance(payload, dict):
                payload["_status_file"] = path.name
                entries.append(payload)
        return entries


async def _stop_process(proc: Process, wait_seconds: float = 3.0) -> None:
    if proc.returncode is not None:
        return
    with contextlib.suppress(ProcessLookupError):
        proc.send_signal(signal.SIGTERM)
    try:
        await asyncio.wait_for(proc.wait(), timeout=wait_seconds)
    except TimeoutError:
        with contextlib.suppress(ProcessLookupError):
            proc.kill()
        await proc.wait()


def _snapshot_for(
    client_pid: int,
    entries: list[dict[str, Any]],
) -> ProgressSnapshot:
    matching = [entry for entry in entries if entry.get("clientPid") == client_pid]
    if not matching:
        return ProgressSnapshot(phase="waiting-for-builder")
    return ProgressSnapshot(
        phase="builder-active",
        goals=tuple(sorted(str(entry.get("_status_file", "")) for entry in matching)),
    )
