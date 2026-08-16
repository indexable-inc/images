import asyncio
import signal
from pathlib import Path

from nix_fast_build.liveness import (
    BIG_PARALLEL_FEATURE,
    BuildLivenessMonitor,
    BuildWatch,
    LivenessPolicy,
    ProgressSnapshot,
    _snapshot_for,
)


class _BlockedProcess:
    pid = 123

    def __init__(self) -> None:
        self.returncode: int | None = None
        self.stopped = asyncio.Event()
        self.signal: int | None = None

    async def wait(self) -> int:
        await self.stopped.wait()
        assert self.returncode is not None
        return self.returncode

    def send_signal(self, signal_number: int) -> None:
        self.signal = signal_number
        self.returncode = -signal_number
        self.stopped.set()

    def kill(self) -> None:
        self.returncode = -signal.SIGKILL
        self.stopped.set()


def _watch(deadline: float = 60) -> BuildWatch:
    return BuildWatch(
        attr="check",
        drv_path="/nix/store/example.drv",
        deadline_seconds=deadline,
        last_progress_at=0,
    )


def test_no_builder_wait_times_out() -> None:
    watch = _watch()

    diagnostic = watch.observe(
        ProgressSnapshot(phase="waiting-for-builder"), 60
    )

    assert diagnostic is not None
    assert "state=waiting-for-builder" in diagnostic
    assert "/nix/store/example.drv" in diagnostic


def test_status_is_correlated_to_exact_client_pid() -> None:
    unrelated = {
        "clientPid": 456,
        "pid": 789,
        "_status_file": "unrelated.json",
    }

    snapshot = _snapshot_for(123, [unrelated])

    assert snapshot == ProgressSnapshot(phase="waiting-for-builder")


def test_no_builder_wait_stops_client(tmp_path: Path) -> None:
    async def run() -> None:
        monitor = BuildLivenessMonitor(
            LivenessPolicy(
                ordinary_seconds=0.01,
                big_parallel_seconds=0.02,
                poll_seconds=0.001,
            ),
            state_dir=tmp_path,
        )
        proc = _BlockedProcess()
        monitor_task = asyncio.create_task(monitor.run())
        return_code, diagnostic = await monitor.wait_for_build(
            proc,
            attr="check",
            drv_path="/nix/store/example.drv",
            required_system_features=frozenset(),
        )
        monitor.stop()
        await monitor_task
        assert return_code == 124
        assert diagnostic is not None
        assert proc.signal == signal.SIGTERM

    asyncio.run(run())


def test_running_builder_is_owned_by_daemon() -> None:
    watch = _watch()
    builder = ProgressSnapshot(
        phase="builder-active", goals=("example-123.json",)
    )

    assert watch.observe(builder, 1) is None
    assert watch.observe(builder, 61) is None


def test_active_long_build_survives() -> None:
    watch = _watch()

    for elapsed in range(1, 601):
        snapshot = ProgressSnapshot(
            phase="builder-active",
            goals=("example-123.json",),
        )
        assert watch.observe(snapshot, float(elapsed)) is None


def test_big_parallel_gets_extended_deadline() -> None:
    policy = LivenessPolicy(ordinary_seconds=60, big_parallel_seconds=300)
    deadline = policy.deadline(frozenset({BIG_PARALLEL_FEATURE}))
    watch = _watch(deadline)

    assert deadline == 300
    assert watch.observe(ProgressSnapshot(phase="waiting-for-builder"), 60) is None
    assert watch.observe(ProgressSnapshot(phase="waiting-for-builder"), 299) is None
    assert watch.observe(ProgressSnapshot(phase="waiting-for-builder"), 300)


def test_policy_renders_daemon_options() -> None:
    assert LivenessPolicy(60, 300).nix_options() == [
        "--option",
        "max-no-progress-time",
        "60",
        "--option",
        "big-parallel-max-no-progress-time",
        "300",
    ]
