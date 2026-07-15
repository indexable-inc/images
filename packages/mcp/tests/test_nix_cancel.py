"""A terminal kernel job must not leave a blocked Nix process tree behind.

Issue #3326 reproduced a Nix client blocked in ``writeToStderr`` after its job
had ended. The fake executable below fills stderr while a descendant holds a
file lock. Cancellation must drain the pipe, kill the isolated process group,
and reap the direct child before the job reports its typed terminal state.
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


def _wait_until_paused(stream: object) -> None:
    transport = getattr(stream, "_transport", None)
    is_reading = getattr(transport, "is_reading", None)
    if not callable(is_reading):
        raise TypeError("subprocess stream has no readable transport")
    deadline = time.monotonic() + 10
    while is_reading():
        if time.monotonic() >= deadline:
            raise TimeoutError("subprocess pipe transport did not pause")
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
chunk = b"x" * 65536
while True:
    os.write(2, chunk)
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
        await self.release.wait()
        self.returncode = -signal.SIGKILL
        return self.returncode

    async def communicate(self) -> tuple[bytes, None]:
        self.reaping.set()
        await self.release.wait()
        self.returncode = -signal.SIGKILL
        return b"", None


def test_job_cancel_drains_stderr_and_reaps_nix_eval_tree(
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

        async def delayed_spawn(
            program: str,
            *args: str,
            cwd: str | None,
            stdout: int,
            stderr: int | None,
            start_new_session: bool,
            pass_fds: tuple[int, ...],
        ) -> asyncio.subprocess.Process:
            proc = await original_spawn(
                program,
                *args,
                cwd=cwd,
                stdout=stdout,
                stderr=stderr,
                start_new_session=start_new_session,
                pass_fds=pass_fds,
            )
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

        def kill_group(target: nix._OwnedProcess) -> None:
            killed.append(target.process.pid)

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


def test_kill_and_reap_drains_a_paused_stderr_pipe() -> None:
    async def scenario() -> int:
        proc = await nix._spawn(
            sys.executable,
            "-c",
            "import os\nchunk = b'x' * 65536\nwhile True: os.write(2, chunk)",
            cwd=None,
            stderr=asyncio.subprocess.PIPE,
        )
        assert proc.process.stderr is not None
        try:
            await asyncio.to_thread(_wait_until_paused, proc.process.stderr)
            cleanup = asyncio.create_task(nix._kill_and_reap(proc))
            done, _ = await asyncio.wait({cleanup}, timeout=5)
            if not done:
                await proc.process.stderr.read()
                await cleanup
                raise AssertionError("reaping waited forever on the paused pipe")
            return cleanup.result()
        finally:
            if proc.process.returncode is None:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(proc.process.pid, signal.SIGKILL)
                proc.release_owner()
                await proc.process.communicate()

    assert asyncio.run(scenario()) == -signal.SIGKILL


def test_owner_pipe_eof_kills_the_process_tree(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> None:
    fake_nix, pid_file, ready_file, lock_file = _fake_tree(monkeypatch, tmp_path)

    async def scenario() -> int:
        proc = await nix._spawn(str(fake_nix), cwd=None, stderr=asyncio.subprocess.PIPE)
        await asyncio.to_thread(_wait_until_ready, pid_file, ready_file)
        proc.release_owner()
        await asyncio.wait_for(proc.process.communicate(), 5)
        assert proc.process.returncode is not None
        return proc.process.returncode

    try:
        assert asyncio.run(scenario()) == -signal.SIGKILL
        _assert_tree_stopped(pid_file, lock_file)
    finally:
        _kill_tree(pid_file)


def test_normal_completion_drains_both_pipes() -> None:
    async def scenario() -> tuple[bytes, bytes]:
        proc = await nix._spawn(
            sys.executable,
            "-c",
            "import sys; print('out'); print('err', file=sys.stderr)",
            cwd=None,
            stderr=asyncio.subprocess.PIPE,
        )
        return await asyncio.wait_for(nix._communicate(proc), 5)

    assert asyncio.run(scenario()) == (b"out\n", b"err\n")


def test_spawn_reports_a_missing_executable_before_backgrounding() -> None:
    async def scenario() -> None:
        with pytest.raises(FileNotFoundError):
            await nix._spawn(
                "ix-mcp-command-that-does-not-exist",
                cwd=None,
                stderr=asyncio.subprocess.PIPE,
            )

    asyncio.run(scenario())


def test_reap_deadline_leaves_a_reporting_watcher(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def scenario() -> tuple[_WaitProcess, list[int]]:
        raw = _WaitProcess()
        owner_read, owner_write = os.pipe()
        os.close(owner_read)
        proc = nix._OwnedProcess(
            process=raw,  # ty: ignore[invalid-argument-type] -- controllable process test double
            owner_fd=owner_write,
        )
        reported: list[int] = []

        monkeypatch.setattr(nix, "_kill_group", lambda _proc: None)
        monkeypatch.setattr(
            nix,
            "_report_late_reap",
            lambda target, _task: reported.append(target.process.pid),
        )

        with pytest.raises(RuntimeError, match="cleanup watcher remains active"):
            await nix._kill_and_reap(proc, timeout=0.01)

        raw.release.set()
        deadline = asyncio.get_running_loop().time() + 1
        while not reported:
            if asyncio.get_running_loop().time() >= deadline:
                raise TimeoutError("late cleanup watcher did not report")
            await asyncio.sleep(0)
        return raw, reported

    raw, reported = asyncio.run(scenario())
    assert raw.returncode == -signal.SIGKILL
    assert reported == [raw.pid]
