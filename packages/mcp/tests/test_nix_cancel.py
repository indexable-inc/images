"""Cancelling a kernel job must stop the process tree owned by ``nix.eval``.

Issue #2737 exposed the gap between cancelling the Python task and cancelling
the subprocess awaited by that task: the job became terminal while both `nix`
and its child kept running. This test uses a fake `nix` executable whose child
holds a file lock, so it proves both the direct process was reaped and the
descendant stopped before the job reports cancellation.
"""

from __future__ import annotations

import asyncio
import contextlib
import fcntl
import os
import pathlib
import signal
import sys
import time

import pytest

import nix
from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict[str, object]) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})
    monkeypatch.setattr(runtime, "_typecheck_enabled", lambda: False)


def _pid_exists(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def _wait_until_ready(pid_file: pathlib.Path, ready_file: pathlib.Path) -> None:
    deadline = time.monotonic() + 10
    while not (pid_file.exists() and ready_file.exists()):
        if time.monotonic() >= deadline:
            raise TimeoutError("fake nix process tree did not start")
        time.sleep(0.01)


def _wait_until_paused(stream: asyncio.StreamReader) -> None:
    deadline = time.monotonic() + 10
    while stream._transport.is_reading():
        if time.monotonic() >= deadline:
            raise TimeoutError("subprocess stdout transport did not pause")
        time.sleep(0.01)


def _fake_tree(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> tuple[pathlib.Path, pathlib.Path, pathlib.Path, pathlib.Path]:
    pid_file = tmp_path / "pids"
    ready_file = tmp_path / "ready"
    lock_file = tmp_path / "child.lock"
    fake_nix = tmp_path / "nix"
    fake_nix.write_text(
        f"""#!{sys.executable}
import fcntl
import os
import pathlib
import time

child = os.fork()
if child == 0:
    with open(os.environ["LOCK_FILE"], "w") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        pathlib.Path(os.environ["READY_FILE"]).write_text("ready")
        while True:
            time.sleep(60)

pathlib.Path(os.environ["PID_FILE"]).write_text(f"{{os.getpid()}} {{child}}")
os.waitpid(child, 0)
"""
    )
    fake_nix.chmod(0o700)
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")
    monkeypatch.setenv("PID_FILE", str(pid_file))
    monkeypatch.setenv("READY_FILE", str(ready_file))
    monkeypatch.setenv("LOCK_FILE", str(lock_file))
    return fake_nix, pid_file, ready_file, lock_file


def _assert_tree_stopped(pid_file: pathlib.Path, lock_file: pathlib.Path) -> None:
    parent = int(pid_file.read_text().split()[0])
    assert not _pid_exists(parent), "the direct nix process was not reaped"
    with lock_file.open("a") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)


def _kill_tree(pid_file: pathlib.Path) -> None:
    if pid_file.exists():
        for value in reversed(pid_file.read_text().split()):
            with contextlib.suppress(ProcessLookupError):
                os.kill(int(value), signal.SIGKILL)


class _EofStream:
    async def read(self, _size: int) -> bytes:
        return b""


class _WaitProcess:
    def __init__(self) -> None:
        self.pid = 424242
        self.returncode: int | None = None
        self.stdout = _EofStream()
        self.wait_calls = 0
        self.waiting = asyncio.Event()
        self.reaping = asyncio.Event()
        self.release = asyncio.Event()

    async def wait(self) -> int:
        self.wait_calls += 1
        self.waiting.set()
        await asyncio.Future()
        raise AssertionError("unreachable")

    async def communicate(self) -> tuple[bytes, None]:
        self.reaping.set()
        await self.release.wait()
        self.returncode = -signal.SIGKILL
        return b"", None


def test_job_cancel_kills_and_reaps_nix_eval_tree(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> None:
    _fake_nix, pid_file, ready_file, lock_file = _fake_tree(monkeypatch, tmp_path)
    _wire(monkeypatch, {"nix": nix})

    async def scenario() -> runtime.Job:
        job = await runtime.__ix_run(
            "await nix.eval('.#slow')",
            budget=0.01,
            session="agent-a",
        )
        await asyncio.to_thread(_wait_until_ready, pid_file, ready_file)
        job.cancel()
        await job.wait(10)
        return job

    try:
        job = asyncio.run(scenario())
        assert job.status == "cancelled"
        _assert_tree_stopped(pid_file, lock_file)
    finally:
        _kill_tree(pid_file)


def test_cancel_during_spawn_setup_recovers_and_kills_tree(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> None:
    fake_nix, pid_file, ready_file, lock_file = _fake_tree(monkeypatch, tmp_path)
    original_spawn = asyncio.create_subprocess_exec

    async def scenario() -> None:
        created = asyncio.Event()
        release = asyncio.Event()

        async def delayed_spawn(*args: object, **kwargs: object) -> asyncio.subprocess.Process:
            proc = await original_spawn(*args, **kwargs)
            created.set()
            await release.wait()
            return proc

        monkeypatch.setattr(nix.asyncio, "create_subprocess_exec", delayed_spawn)
        task = asyncio.create_task(
            nix._spawn(str(fake_nix), cwd=None, stderr=asyncio.subprocess.PIPE)
        )
        try:
            await asyncio.wait_for(created.wait(), 5)
            await asyncio.to_thread(_wait_until_ready, pid_file, ready_file)
            task.cancel()
            release.set()
            with pytest.raises(asyncio.CancelledError):
                await task
        finally:
            release.set()
            if not task.done():
                task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await task

    try:
        asyncio.run(scenario())
        _assert_tree_stopped(pid_file, lock_file)
    finally:
        _kill_tree(pid_file)


def test_run_cancel_during_wait_reaps_despite_repeated_cancel(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def scenario() -> tuple[_WaitProcess, list[int]]:
        proc = _WaitProcess()
        killed: list[int] = []

        async def spawn(*_args: object, **_kwargs: object) -> _WaitProcess:
            return proc

        def kill_group(target: _WaitProcess) -> None:
            killed.append(target.pid)

        monkeypatch.setattr(nix.asyncio, "create_subprocess_exec", spawn)
        monkeypatch.setattr(nix, "_kill_group", kill_group)

        task = asyncio.create_task(nix.run(["build", ".#slow"], live=False))
        try:
            await asyncio.wait_for(proc.waiting.wait(), 1)
            task.cancel()
            await asyncio.wait_for(proc.reaping.wait(), 1)
            task.cancel()
            await asyncio.sleep(0)
            assert not task.done(), "a repeated cancel bypassed process reaping"
            proc.release.set()
            with pytest.raises(asyncio.CancelledError):
                await task
        finally:
            proc.release.set()
            if not task.done():
                task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await task
        return proc, killed

    proc, killed = asyncio.run(scenario())
    assert killed == [proc.pid]
    assert proc.returncode == -signal.SIGKILL


def test_kill_and_reap_drains_a_paused_pipe() -> None:
    async def scenario() -> int:
        proc = await asyncio.create_subprocess_exec(
            sys.executable,
            "-c",
            "import os\nchunk = b'x' * 65536\nwhile True: os.write(1, chunk)",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            start_new_session=True,
        )
        assert proc.stdout is not None
        try:
            await asyncio.to_thread(_wait_until_paused, proc.stdout)
            cleanup = asyncio.create_task(nix._kill_and_reap(proc))
            done, _ = await asyncio.wait({cleanup}, timeout=5)
            if not done:
                await proc.stdout.read()
                await cleanup
                raise AssertionError("reaping waited forever on the paused pipe")
            return cleanup.result()
        finally:
            if proc.returncode is None:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(proc.pid, signal.SIGKILL)
                await proc.communicate()

    assert asyncio.run(scenario()) == -signal.SIGKILL
