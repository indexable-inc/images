"""Terminal kernel jobs must leave no Nix process identity or pipe reader."""

from __future__ import annotations

import asyncio
import contextlib
import fcntl
import os
import pathlib
import sys
import textwrap
import time
import warnings
from collections.abc import Awaitable
from dataclasses import dataclass

import psutil
import pytest

import nix
from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict[str, object]) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})
    monkeypatch.setattr(runtime, "_typecheck_enabled", lambda: False)


def _wait_for(*paths: pathlib.Path, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while not all(path.exists() for path in paths):
        if time.monotonic() >= deadline:
            missing = ", ".join(str(path) for path in paths if not path.exists())
            raise TimeoutError(f"process fixture did not create {missing}")
        time.sleep(0.01)


def _write_executable(path: pathlib.Path, source: str) -> pathlib.Path:
    path.write_text(textwrap.dedent(source))
    path.chmod(0o700)
    return path


@dataclass(frozen=True, slots=True)
class _Identity:
    pid: int
    created: float

    @classmethod
    def capture(cls, pid: int) -> _Identity:
        process = psutil.Process(pid)
        return cls(pid=pid, created=process.create_time())

    def process(self) -> psutil.Process | None:
        try:
            process = psutil.Process(self.pid)
            if process.create_time() != self.created:
                return None
            return process
        except (psutil.NoSuchProcess, psutil.ZombieProcess):
            return None

    def running(self) -> bool:
        process = self.process()
        if process is None:
            return False
        try:
            return process.is_running() and process.status() != psutil.STATUS_ZOMBIE
        except (psutil.NoSuchProcess, psutil.ZombieProcess):
            return False


def _identities(pid_file: pathlib.Path) -> tuple[_Identity, ...]:
    return tuple(
        _Identity.capture(int(value)) for value in pid_file.read_text().split()
    )


def _cleanup_identities(identities: tuple[_Identity, ...]) -> None:
    processes = [
        process for identity in reversed(identities) if (process := identity.process())
    ]
    for process in processes:
        with contextlib.suppress(psutil.NoSuchProcess, psutil.ZombieProcess):
            process.kill()
    psutil.wait_procs(processes, timeout=2)


def _assert_stopped(identities: tuple[_Identity, ...], lock_file: pathlib.Path) -> None:
    deadline = time.monotonic() + 3
    while any(identity.running() for identity in identities):
        if time.monotonic() >= deadline:
            alive = [str(identity.pid) for identity in identities if identity.running()]
            raise AssertionError(
                f"process identities survived cleanup: {', '.join(alive)}"
            )
        time.sleep(0.01)
    with lock_file.open("a") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)


def _blocked_tree(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> tuple[pathlib.Path, pathlib.Path, pathlib.Path, pathlib.Path, pathlib.Path]:
    pid_file = tmp_path / "pids"
    ready_file = tmp_path / "ready"
    blocked_file = tmp_path / "write-blocked"
    lock_file = tmp_path / "child.lock"
    fake_nix = _write_executable(
        tmp_path / "nix",
        f"""\
        #!{sys.executable}
        import fcntl
        import os
        import pathlib
        import time

        child = os.fork()
        if child == 0:
            os.setsid()
            with open(os.environ["LOCK_FILE"], "w") as lock:
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
                pathlib.Path(os.environ["READY_FILE"]).write_text("ready")
                while True:
                    time.sleep(60)

        pathlib.Path(os.environ["PID_FILE"]).write_text(f"{{os.getpid()}} {{child}}")
        flags = fcntl.fcntl(2, fcntl.F_GETFL)
        fcntl.fcntl(2, fcntl.F_SETFL, flags | os.O_NONBLOCK)
        chunk = b"x" * 65536
        while True:
            try:
                os.write(2, chunk)
            except BlockingIOError:
                break
        pathlib.Path(os.environ["BLOCKED_FILE"]).write_text("stderr pipe saturated")
        fcntl.fcntl(2, fcntl.F_SETFL, flags)
        while True:
            os.write(2, chunk)
        """,
    )
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")
    monkeypatch.setenv("PID_FILE", str(pid_file))
    monkeypatch.setenv("READY_FILE", str(ready_file))
    monkeypatch.setenv("BLOCKED_FILE", str(blocked_file))
    monkeypatch.setenv("LOCK_FILE", str(lock_file))
    return fake_nix, pid_file, ready_file, blocked_file, lock_file


def _stream_is_paused(stream: asyncio.StreamReader) -> bool:
    transport = getattr(stream, "_transport", None)
    is_reading = getattr(transport, "is_reading", None)
    if not callable(is_reading):
        raise TypeError("subprocess stream has no readable transport")
    return not is_reading()


async def _wait_until_stream_paused(stream: asyncio.StreamReader) -> None:
    deadline = asyncio.get_running_loop().time() + 2
    while not _stream_is_paused(stream):
        if asyncio.get_running_loop().time() >= deadline:
            raise TimeoutError("stderr transport did not reach backpressure")
        await asyncio.sleep(0.01)


async def _reap_direct(process: asyncio.subprocess.Process) -> None:
    if process.returncode is None:
        with contextlib.suppress(ProcessLookupError):
            process.kill()
    with contextlib.suppress(TimeoutError):
        await asyncio.wait_for(process.wait(), 2)


def test_job_cancel_reaps_a_setsid_tree_blocked_in_stderr_write(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> None:
    _fake, pid_file, ready_file, blocked_file, lock_file = _blocked_tree(
        monkeypatch, tmp_path
    )
    _wire(monkeypatch, {"nix": nix})
    original_spawn = asyncio.create_subprocess_exec
    identities: tuple[_Identity, ...] = ()

    async def scenario() -> runtime.Job:
        created = asyncio.Event()
        release = asyncio.Event()
        spawned: list[asyncio.subprocess.Process] = []

        async def delayed_spawn(
            program: str,
            *args: str,
            cwd: str | None,
            stdout: int,
            stderr: int | None,
            start_new_session: bool = False,
            pass_fds: tuple[int, ...] = (),
        ) -> asyncio.subprocess.Process:
            process = await original_spawn(
                program,
                *args,
                cwd=cwd,
                stdout=stdout,
                stderr=stderr,
                start_new_session=start_new_session,
                pass_fds=pass_fds,
            )
            spawned.append(process)
            created.set()
            await release.wait()
            return process

        monkeypatch.setattr(nix.asyncio, "create_subprocess_exec", delayed_spawn)
        job = await runtime.__ix_run(
            "await nix.eval('.#slow')",
            budget=0.01,
            session="agent-a",
        )
        try:
            await asyncio.to_thread(_wait_for, pid_file, ready_file, blocked_file)
            await asyncio.wait_for(created.wait(), 1)
            assert spawned[0].stderr is not None
            await _wait_until_stream_paused(spawned[0].stderr)
            nonlocal identities
            identities = _identities(pid_file)
            job.cancel()
            release.set()
            await asyncio.sleep(0)
            job.cancel()
            await job.wait(10)
            return job
        finally:
            release.set()
            for process in spawned:
                await _reap_direct(process)

    try:
        job = asyncio.run(scenario())
        assert job.status == "cancelled"
        _assert_stopped(identities, lock_file)
    finally:
        _cleanup_identities(identities)


def test_target_has_no_supervisor_child(tmp_path: pathlib.Path) -> None:
    identity_file = tmp_path / "identity"
    no_child_file = tmp_path / "no-child"
    target = _write_executable(
        tmp_path / "target",
        f"""\
        #!{sys.executable}
        import os
        import pathlib
        import time

        pathlib.Path({str(identity_file)!r}).write_text(f"{{os.getpid()}} {{os.getppid()}}")
        try:
            os.waitpid(-1, 0)
        except ChildProcessError:
            pathlib.Path({str(no_child_file)!r}).write_text("no child supervisor")
        while True:
            time.sleep(60)
        """,
    )

    async def scenario() -> tuple[int, int, int, int]:
        process = await nix._spawn(
            str(target), cwd=None, stderr=asyncio.subprocess.PIPE
        )
        try:
            await asyncio.to_thread(_wait_for, identity_file, no_child_file, timeout=2)
            target_pid, parent_pid = map(int, identity_file.read_text().split())
            supervisor_pid = process.process.pid
            return supervisor_pid, os.getsid(supervisor_pid), target_pid, parent_pid
        finally:
            await nix._kill_and_reap(process)

    supervisor_pid, supervisor_session, target_pid, parent_pid = asyncio.run(scenario())
    assert supervisor_session == supervisor_pid
    assert target_pid != supervisor_pid
    if sys.platform == "linux":
        assert parent_pid == supervisor_pid
    else:
        assert parent_pid != supervisor_pid


def test_normal_exit_reaps_a_tracked_setsid_descendant(tmp_path: pathlib.Path) -> None:
    pid_file = tmp_path / "pids"
    ready_file = tmp_path / "ready"
    lock_file = tmp_path / "child.lock"
    target = _write_executable(
        tmp_path / "target",
        f"""\
        #!{sys.executable}
        import fcntl
        import os
        import pathlib
        import time

        child = os.fork()
        if child == 0:
            os.setsid()
            with open({str(lock_file)!r}, "w") as lock:
                fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
                pathlib.Path({str(ready_file)!r}).write_text("ready")
                while True:
                    time.sleep(60)
        pathlib.Path({str(pid_file)!r}).write_text(f"{{os.getpid()}} {{child}}")
        while not pathlib.Path({str(ready_file)!r}).exists():
            time.sleep(0.01)
        time.sleep(0.2)
        """,
    )
    identities: tuple[_Identity, ...] = ()

    async def scenario() -> tuple[bytes, bytes]:
        process = await nix._spawn(
            str(target), cwd=None, stderr=asyncio.subprocess.PIPE
        )
        await asyncio.to_thread(_wait_for, pid_file, ready_file)
        nonlocal identities
        identities = _identities(pid_file)
        return await asyncio.wait_for(nix._communicate(process), 3)

    try:
        out, err = asyncio.run(scenario())
        assert (out, err) == (b"", b"")
        _assert_stopped(identities, lock_file)
    finally:
        _cleanup_identities(identities)


def test_darwin_coalition_reaps_a_fast_closerange_double_fork(
    tmp_path: pathlib.Path,
) -> None:
    if sys.platform != "darwin":
        pytest.skip("Darwin uses launchd coalition containment instead of a subreaper")

    identity_file = tmp_path / "identity"
    lock_file = tmp_path / "grandchild.lock"
    target = _write_executable(
        tmp_path / "target",
        f"""\
        #!{sys.executable}
        import fcntl
        import os
        import pathlib
        import psutil
        import time

        time.sleep(0.03)
        child = os.fork()
        if child == 0:
            os.setsid()
            grandchild = os.fork()
            if grandchild == 0:
                os.closerange(3, 1024)
                process = psutil.Process()
                with open({str(lock_file)!r}, "w") as lock:
                    fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
                    pathlib.Path({str(identity_file)!r}).write_text(
                        f"{{process.pid}} {{process.create_time()}}"
                    )
                    while True:
                        time.sleep(60)
            os._exit(0)
        os._exit(0)
        """,
    )
    identity: _Identity | None = None

    async def scenario() -> tuple[bytes, bytes]:
        process = await nix._spawn(
            str(target), cwd=None, stderr=asyncio.subprocess.PIPE
        )
        return await asyncio.wait_for(nix._communicate(process), 3)

    try:
        assert asyncio.run(scenario()) == (b"", b"")
        pid, created = identity_file.read_text().split()
        identity = _Identity(pid=int(pid), created=float(created))
        _assert_stopped((identity,), lock_file)
    finally:
        if identity is not None:
            _cleanup_identities((identity,))


def test_darwin_supervisor_waits_after_target_closes_output(
    tmp_path: pathlib.Path,
) -> None:
    if sys.platform != "darwin":
        pytest.skip("Darwin relays launchd-owned output pipes")

    ready_file = tmp_path / "ready"
    target = _write_executable(
        tmp_path / "target",
        f"""\
        #!{sys.executable}
        import os
        import pathlib
        import time

        os.close(1)
        os.close(2)
        pathlib.Path({str(ready_file)!r}).write_text("ready")
        time.sleep(0.3)
        raise SystemExit(7)
        """,
    )

    async def scenario() -> tuple[tuple[bytes, bytes], int, float]:
        process = await nix._spawn(
            str(target), cwd=None, stderr=asyncio.subprocess.PIPE
        )
        await asyncio.to_thread(_wait_for, ready_file)
        started = time.monotonic()
        output = await asyncio.wait_for(nix._communicate(process), 3)
        elapsed = time.monotonic() - started
        assert process.process.returncode is not None
        return output, process.process.returncode, elapsed

    output, returncode, elapsed = asyncio.run(scenario())
    assert output == (b"", b"")
    assert returncode == 7
    assert elapsed >= 0.2


def test_darwin_status_is_reread_after_wrapper_exit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from nix import _supervise

    statuses = iter((None, 7 << 8))
    job = object.__new__(_supervise._DarwinJob)
    job.leader = (424242, 1.0)
    monkeypatch.setattr(
        _supervise._DarwinJob,
        "read_status",
        lambda _job: next(statuses),
    )
    monkeypatch.setattr(
        _supervise._DarwinJob,
        "leader_running",
        lambda _job: False,
    )

    assert job.terminal_status() == 7 << 8


def test_pidfd_capture_revalidates_identity_after_open(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from nix import _supervise

    pidfd, peer = os.pipe()
    events: list[str] = []

    class ChangedProcess:
        pid = 424242

        def create_time(self) -> float:
            events.append("identity")
            return float(events.count("identity"))

        def is_running(self) -> bool:
            events.append("running")
            return True

    def open_pidfd(pid: int) -> int:
        assert pid == ChangedProcess.pid
        events.append("pidfd")
        return pidfd

    monkeypatch.setattr(_supervise, "_PIDFD_OPEN", open_pidfd)
    try:
        with pytest.raises(psutil.NoSuchProcess):
            _supervise._Member.capture(
                ChangedProcess(),  # ty: ignore[invalid-argument-type] identity race double
            )
        with pytest.raises(OSError, match=r"\[Errno 9\]"):
            os.fstat(pidfd)
        assert events == ["identity", "pidfd", "running", "identity"]
    finally:
        with contextlib.suppress(OSError):
            os.close(pidfd)
        os.close(peer)


class _LaunchAbort(BaseException):
    pass


def _open_fds() -> set[int]:
    root = (
        pathlib.Path("/proc/self/fd")
        if pathlib.Path("/proc/self/fd").exists()
        else pathlib.Path("/dev/fd")
    )
    return {int(path.name) for path in root.iterdir() if path.name.isdigit()}


def test_base_exception_during_launch_closes_owner_and_reaps(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> None:
    ready_file = tmp_path / "ready"
    target = _write_executable(
        tmp_path / "target",
        f"""\
        #!{sys.executable}
        import pathlib
        import time
        pathlib.Path({str(ready_file)!r}).write_text("ready")
        while True:
            time.sleep(60)
        """,
    )
    original_spawn = asyncio.create_subprocess_exec
    original_shield = asyncio.shield
    before = _open_fds()

    async def scenario() -> asyncio.subprocess.Process:
        created = asyncio.Event()
        release = asyncio.Event()
        spawned: list[asyncio.subprocess.Process] = []
        shield_calls = 0

        async def delayed_spawn(
            program: str,
            *args: str,
            cwd: str | None,
            stdout: int,
            stderr: int | None,
            start_new_session: bool = False,
            pass_fds: tuple[int, ...] = (),
        ) -> asyncio.subprocess.Process:
            process = await original_spawn(
                program,
                *args,
                cwd=cwd,
                stdout=stdout,
                stderr=stderr,
                start_new_session=start_new_session,
                pass_fds=pass_fds,
            )
            spawned.append(process)
            created.set()
            await release.wait()
            return process

        def abort_first_shield(awaitable: Awaitable[object]) -> Awaitable[object]:
            nonlocal shield_calls
            shield_calls += 1
            if shield_calls != 1:
                return original_shield(awaitable)

            async def abort() -> None:
                await created.wait()
                await asyncio.to_thread(_wait_for, ready_file)
                release.set()
                raise _LaunchAbort

            return abort()

        monkeypatch.setattr(nix.asyncio, "create_subprocess_exec", delayed_spawn)
        monkeypatch.setattr(nix.asyncio, "shield", abort_first_shield)
        try:
            with pytest.raises(_LaunchAbort):
                await nix._spawn(str(target), cwd=None, stderr=asyncio.subprocess.PIPE)
            assert spawned[0].returncode is not None
            return spawned[0]
        finally:
            release.set()
            for process in spawned:
                await _reap_direct(process)

    asyncio.run(scenario())
    leaked = _open_fds() - before
    try:
        assert not leaked, f"launch leaked file descriptors: {sorted(leaked)}"
    finally:
        for fd in leaked:
            with contextlib.suppress(OSError):
                os.close(fd)


class _NeverProcess:
    def __init__(self) -> None:
        self.pid = 424242
        self.returncode: int | None = None
        self.stdout = None
        self.stderr = None
        self.drain_started = asyncio.Event()
        self.wait_started = asyncio.Event()
        self.drain_stopped = asyncio.Event()
        self.wait_stopped = asyncio.Event()
        self.killed = False

    async def communicate(self) -> tuple[bytes, None]:
        self.drain_started.set()
        try:
            await asyncio.Event().wait()
            raise AssertionError("unreachable")
        finally:
            self.drain_stopped.set()

    async def wait(self) -> int:
        self.wait_started.set()
        try:
            await asyncio.Event().wait()
            raise AssertionError("unreachable")
        finally:
            self.wait_stopped.set()

    def kill(self) -> None:
        self.killed = True


def test_reap_deadline_stops_every_cleanup_task() -> None:
    async def scenario() -> _NeverProcess:
        raw = _NeverProcess()
        owner_read, owner_write = nix._owner_pipe()
        os.close(owner_read)
        process = nix._OwnedProcess(
            process=raw,  # ty: ignore[invalid-argument-type] controllable process double
            owner_fd=owner_write,
        )
        current = asyncio.current_task()
        before = {task for task in asyncio.all_tasks() if task is not current}
        with pytest.raises(RuntimeError, match="could not be force-stopped"):
            await nix._kill_and_reap(process, timeout=0.01)
        await asyncio.sleep(0)
        after = {task for task in asyncio.all_tasks() if task is not current}
        assert after <= before, "cleanup left an unbounded reader or waiter task"
        assert raw.killed
        assert raw.drain_started.is_set()
        assert raw.wait_started.is_set()
        assert raw.drain_stopped.is_set()
        assert raw.wait_stopped.is_set()
        return raw

    asyncio.run(scenario())


def test_worktree_cannot_shadow_the_supervisor_helper(tmp_path: pathlib.Path) -> None:
    marker = tmp_path / "shadow-imported"
    (tmp_path / "nix.py").write_text(
        f"import pathlib\npathlib.Path({str(marker)!r}).write_text('imported')\n"
    )

    async def scenario() -> tuple[bytes, bytes]:
        process = await nix._spawn(
            sys.executable,
            "-c",
            "import sys; print('out'); print('err', file=sys.stderr)",
            cwd=str(tmp_path),
            stderr=asyncio.subprocess.PIPE,
        )
        return await asyncio.wait_for(nix._communicate(process), 5)

    assert asyncio.run(scenario()) == (b"out\n", b"err\n")
    assert not marker.exists()


def test_relative_path_entry_resolves_from_target_cwd(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
) -> None:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    _write_executable(
        bin_dir / "worktree-command",
        f"#!{sys.executable}\nprint('worktree executable')\n",
    )
    monkeypatch.setenv("PATH", "bin")

    async def scenario() -> tuple[bytes, bytes]:
        process = await nix._spawn(
            "worktree-command",
            cwd=str(tmp_path),
            stderr=asyncio.subprocess.PIPE,
        )
        return await asyncio.wait_for(nix._communicate(process), 5)

    assert asyncio.run(scenario()) == (b"worktree executable\n", b"")


def test_spawn_preserves_an_owner_pipe_allocated_as_stdin() -> None:
    async def scenario() -> tuple[bytes, bytes]:
        saved_stdin = os.dup(0)
        os.close(0)
        try:
            process = await nix._spawn(
                sys.executable,
                "-c",
                "import sys; print('out'); print('err', file=sys.stderr)",
                cwd=None,
                stderr=asyncio.subprocess.PIPE,
            )
            return await asyncio.wait_for(nix._communicate(process), 5)
        finally:
            os.dup2(saved_stdin, 0)
            os.close(saved_stdin)

    assert asyncio.run(scenario()) == (b"out\n", b"err\n")


def test_unrelated_fork_cannot_extend_owner_lifetime(
    tmp_path: pathlib.Path,
) -> None:
    pid_file = tmp_path / "pid"
    ready_file = tmp_path / "ready"
    lock_file = tmp_path / "target.lock"
    target = _write_executable(
        tmp_path / "target",
        f"""\
        #!{sys.executable}
        import fcntl
        import os
        import pathlib
        import time

        pathlib.Path({str(pid_file)!r}).write_text(str(os.getpid()))
        with open({str(lock_file)!r}, "w") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
            pathlib.Path({str(ready_file)!r}).write_text("ready")
            while True:
                time.sleep(60)
        """,
    )
    identities: tuple[_Identity, ...] = ()

    async def scenario() -> int:
        process = await nix._spawn(
            str(target), cwd=None, stderr=asyncio.subprocess.PIPE
        )
        holder = -1
        holder_ready_read, holder_ready_write = os.pipe()
        holder_release_read, holder_release_write = os.pipe()
        try:
            await asyncio.to_thread(_wait_for, pid_file, ready_file)
            nonlocal identities
            identities = (_Identity.capture(int(pid_file.read_text())),)
            with warnings.catch_warnings():
                warnings.filterwarnings(
                    "ignore",
                    message=r"This process .* is multi-threaded, use of fork\(\) may lead to deadlocks in the child\.",
                    category=DeprecationWarning,
                )
                holder = os.fork()
            if holder == 0:
                os.close(holder_ready_read)
                os.close(holder_release_write)
                os.write(holder_ready_write, b"ready")
                os.close(holder_ready_write)
                os.read(holder_release_read, 1)
                os._exit(0)
            os.close(holder_ready_write)
            os.close(holder_release_read)
            await asyncio.to_thread(os.read, holder_ready_read, 5)
            holder_identity = _Identity.capture(holder)
            result = await asyncio.wait_for(
                nix._kill_and_reap(process, timeout=1),
                3,
            )
            assert holder_identity.running(), "unrelated fork exited with the target"
            return result
        finally:
            with contextlib.suppress(OSError):
                os.close(holder_ready_read)
            if holder > 0:
                with contextlib.suppress(BrokenPipeError):
                    os.write(holder_release_write, b"stop")
                os.close(holder_release_write)
                await asyncio.to_thread(os.waitpid, holder, 0)
            else:
                os.close(holder_ready_write)
                os.close(holder_release_read)
                os.close(holder_release_write)
            if process.process.returncode is None:
                await nix._kill_and_reap(process)

    try:
        assert asyncio.run(scenario()) != 0
        _assert_stopped(identities, lock_file)
    finally:
        _cleanup_identities(identities)


def test_normal_completion_drains_both_pipes() -> None:
    async def scenario() -> tuple[bytes, bytes]:
        process = await nix._spawn(
            sys.executable,
            "-c",
            "import sys; print('out'); print('err', file=sys.stderr)",
            cwd=None,
            stderr=asyncio.subprocess.PIPE,
        )
        return await asyncio.wait_for(nix._communicate(process), 5)

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
