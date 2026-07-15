"""Run one command beneath a lifetime-bound process supervisor."""

from __future__ import annotations

import contextlib
import ctypes
import json
import os
import plistlib
import selectors
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass
from dataclasses import field
from pathlib import Path
from typing import NoReturn

import psutil

_WATCH_SECONDS = 0.1
_SHUTDOWN_POLL_SECONDS = 0.01
_SHUTDOWN_SECONDS = 5.0
_PR_SET_CHILD_SUBREAPER = 36
_PIDFD_OPEN = getattr(os, "pidfd_open", None)
_PIDFD_SEND_SIGNAL = getattr(signal, "pidfd_send_signal", None)
# XNU doc/observability/coalitions.md makes launchd coalition membership
# immutable across fork and exec. These selectors come from
# bsd/sys/proc_info_private.h so cleanup can address that stable identity.
_PROC_PIDCOALITIONINFO = 20
_LISTCOALITIONS_SINGLE_TYPE = 2
_COALITION_TYPE_RESOURCE = 0
_DARWIN_LAUNCH_SECONDS = 3.0


@dataclass(slots=True)
class _Member:
    """One process identity captured before any signal can be sent."""

    process: psutil.Process
    created: float
    pidfd: int | None

    @classmethod
    def capture(cls, process: psutil.Process) -> _Member:
        created = process.create_time()
        pidfd = _PIDFD_OPEN(process.pid) if _PIDFD_OPEN is not None else None
        if pidfd is not None:
            try:
                if not process.is_running() or process.create_time() != created:
                    raise psutil.NoSuchProcess(process.pid)
            except BaseException:
                os.close(pidfd)
                raise
        return cls(process=process, created=created, pidfd=pidfd)

    @property
    def identity(self) -> tuple[int, float]:
        return self.process.pid, self.created

    def running(self) -> bool:
        if self.pidfd is not None:
            return not _pidfd_exited(self.pidfd)
        try:
            return (
                self.process.is_running()
                and self.process.status() != psutil.STATUS_ZOMBIE
            )
        except (psutil.NoSuchProcess, psutil.ZombieProcess):
            return False

    def kill(self) -> None:
        try:
            if self.pidfd is not None:
                assert _PIDFD_SEND_SIGNAL is not None
                _PIDFD_SEND_SIGNAL(self.pidfd, signal.SIGKILL)
            else:
                # psutil validates PID plus creation time immediately before the
                # signal. A reused PID therefore cannot become a cleanup target.
                self.process.kill()
        except (ProcessLookupError, psutil.NoSuchProcess, psutil.ZombieProcess):
            pass

    def close(self) -> None:
        if self.pidfd is not None:
            os.close(self.pidfd)
            self.pidfd = None


class _ProcPidCoalitionInfo(ctypes.Structure):
    _fields_ = [
        ("coalition_ids", ctypes.c_uint64 * 2),
        ("reserved", ctypes.c_uint64 * 3),
    ]


class _ProcCoalitionInfo(ctypes.Structure):
    _fields_ = [
        ("coalition_id", ctypes.c_uint64),
        ("coalition_type", ctypes.c_uint32),
        ("task_count", ctypes.c_uint32),
    ]


@dataclass(slots=True)
class _DarwinProc:
    """Query immutable launchd coalition membership through libproc."""

    library: ctypes.CDLL

    @classmethod
    def create(cls) -> _DarwinProc:
        library = ctypes.CDLL("libproc.dylib", use_errno=True)
        library.proc_pidinfo.argtypes = [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_uint64,
            ctypes.c_void_p,
            ctypes.c_int,
        ]
        library.proc_pidinfo.restype = ctypes.c_int
        library.proc_listcoalitions.argtypes = [
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_int,
        ]
        library.proc_listcoalitions.restype = ctypes.c_int
        return cls(library=library)

    def coalitions(self, pid: int) -> tuple[int, int] | None:
        info = _ProcPidCoalitionInfo()
        written = self.library.proc_pidinfo(
            pid,
            _PROC_PIDCOALITIONINFO,
            0,
            ctypes.byref(info),
            ctypes.sizeof(info),
        )
        if written <= 0:
            return None
        if written != ctypes.sizeof(info):
            raise RuntimeError(
                f"libproc returned {written} bytes of coalition data for pid {pid}"
            )
        return int(info.coalition_ids[0]), int(info.coalition_ids[1])

    def task_count(self, coalition_id: int) -> int:
        list_coalitions = self.library.proc_listcoalitions
        required = list_coalitions(
            _LISTCOALITIONS_SINGLE_TYPE,
            _COALITION_TYPE_RESOURCE,
            None,
            0,
        )
        if required <= 0:
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))
        size = max(required * 2, 4096)
        while True:
            count = size // ctypes.sizeof(_ProcCoalitionInfo)
            buffer = (_ProcCoalitionInfo * count)()
            written = list_coalitions(
                _LISTCOALITIONS_SINGLE_TYPE,
                _COALITION_TYPE_RESOURCE,
                buffer,
                size,
            )
            if written <= 0:
                error = ctypes.get_errno()
                raise OSError(error, os.strerror(error))
            if written < size:
                break
            size *= 2
        for info in buffer[: written // ctypes.sizeof(_ProcCoalitionInfo)]:
            if info.coalition_id == coalition_id:
                return int(info.task_count)
        return 0


def _json_temporary(path: Path) -> Path:
    return path.with_name(f".{path.name}.tmp")


def _write_private_json(path: Path, value: object) -> None:
    temporary = _json_temporary(path)
    descriptor = os.open(
        temporary,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL,
        0o600,
    )
    try:
        try:
            stream = os.fdopen(descriptor, "w")
        except BaseException:
            with contextlib.suppress(OSError):
                os.close(descriptor)
            raise
        with stream:
            json.dump(value, stream)
        temporary.replace(path)
    except BaseException:
        with contextlib.suppress(FileNotFoundError):
            temporary.unlink()
        raise


@dataclass(frozen=True, slots=True)
class _DarwinTargetConfig:
    argv: list[str]
    cwd: str
    environment: dict[str, str]
    ready: Path
    status: Path

    def write(self, path: Path) -> None:
        _write_private_json(
            path,
            {
                "argv": self.argv,
                "cwd": self.cwd,
                "environment": self.environment,
                "ready": str(self.ready),
                "status": str(self.status),
            },
        )

    @classmethod
    def read(cls, path: Path) -> _DarwinTargetConfig:
        with path.open() as stream:
            value = json.load(stream)
        if not isinstance(value, dict):
            raise TypeError("Darwin target config must be a JSON object")
        argv = value.get("argv")
        cwd = value.get("cwd")
        environment = value.get("environment")
        ready = value.get("ready")
        status = value.get("status")
        if not isinstance(argv, list) or not all(
            isinstance(argument, str) for argument in argv
        ):
            raise TypeError("Darwin target argv must contain only strings")
        if not isinstance(cwd, str):
            raise TypeError("Darwin target cwd must be a string")
        if not isinstance(environment, dict) or not all(
            isinstance(key, str) and isinstance(item, str)
            for key, item in environment.items()
        ):
            raise TypeError("Darwin target environment must map strings to strings")
        if not isinstance(ready, str) or not isinstance(status, str):
            raise TypeError("Darwin target state paths must be strings")
        return cls(
            argv=argv,
            cwd=cwd,
            environment=environment,
            ready=Path(ready),
            status=Path(status),
        )


def _write_all(descriptor: int, data: bytes) -> None:
    while data:
        data = data[os.write(descriptor, data) :]


def _start_relay(owner_fd: int, path: Path, output_fd: int) -> int:
    relay = os.fork()
    if relay != 0:
        return relay
    try:
        os.close(owner_fd)
        with contextlib.suppress(OSError):
            os.close(2 if output_fd == 1 else 1)
        os.setsid()
        descriptor = os.open(path, os.O_RDONLY)
        try:
            while chunk := os.read(descriptor, 65536):
                _write_all(output_fd, chunk)
        finally:
            os.close(descriptor)
    except BaseException:
        os._exit(125)
    os._exit(0)


@dataclass(slots=True)
class _DarwinJob:
    """A launchd job whose immutable coalition contains the whole target tree."""

    root: Path
    label: str
    domain: str
    config_path: Path
    ready_path: Path
    status_path: Path
    stdout_path: Path
    stderr_path: Path
    plist_path: Path
    proc: _DarwinProc
    relays: list[int] = field(default_factory=list)
    coalitions: tuple[int, int] | None = None
    leader: tuple[int, float] | None = None
    bootstrap_attempted: bool = False
    loaded: bool = False
    removed: bool = False

    @classmethod
    def allocate(cls, argv: list[str]) -> _DarwinJob:
        proc = _DarwinProc.create()
        root = Path(tempfile.mkdtemp(prefix="ix-nix-supervisor-"))
        label = f"com.indexable.nix-supervisor.{os.getpid()}.{uuid.uuid4().hex}"
        job = cls(
            root=root,
            label=label,
            domain=f"gui/{os.getuid()}",
            config_path=root / "target.json",
            ready_path=root / "ready.json",
            status_path=root / "status.json",
            stdout_path=root / "stdout",
            stderr_path=root / "stderr",
            plist_path=root / "job.plist",
            proc=proc,
        )
        try:
            os.mkfifo(job.stdout_path, 0o600)
            os.mkfifo(job.stderr_path, 0o600)
            _DarwinTargetConfig(
                argv=argv,
                cwd=str(Path.cwd()),
                environment=dict(os.environ),
                ready=job.ready_path,
                status=job.status_path,
            ).write(job.config_path)
            job.plist_path.write_bytes(
                plistlib.dumps(
                    {
                        "AbandonProcessGroup": True,
                        "KeepAlive": False,
                        "Label": job.label,
                        "ProgramArguments": [
                            sys.executable,
                            str(Path(__file__).resolve()),
                            "--darwin-target",
                            str(job.config_path),
                        ],
                        "RunAtLoad": True,
                        "StandardErrorPath": str(job.stderr_path),
                        "StandardOutPath": str(job.stdout_path),
                        "Umask": 0o077,
                        "WorkingDirectory": str(job.root),
                    },
                    fmt=plistlib.FMT_XML,
                )
            )
            job.plist_path.chmod(0o600)
        except BaseException:
            job.close()
            raise
        return job

    @property
    def target(self) -> str:
        return f"{self.domain}/{self.label}"

    def start(self, owner_fd: int) -> None:
        self.relays.append(_start_relay(owner_fd, self.stdout_path, 1))
        self.relays.append(_start_relay(owner_fd, self.stderr_path, 2))
        self.bootstrap_attempted = True
        result = subprocess.run(
            ["/bin/launchctl", "bootstrap", self.domain, str(self.plist_path)],
            check=False,
            capture_output=True,
            text=True,
            timeout=_DARWIN_LAUNCH_SECONDS,
        )
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip() or "no detail"
            raise RuntimeError(f"launchctl could not bootstrap {self.label}: {detail}")
        self.loaded = True

    def read_status(self) -> int | None:
        if not self.status_path.exists():
            return None
        with self.status_path.open() as stream:
            value = json.load(stream)
        if not isinstance(value, dict) or not isinstance(value.get("status"), int):
            raise TypeError("Darwin target status must contain an integer status")
        return value["status"]

    def load_coalitions(self) -> None:
        if self.coalitions is not None or not self.ready_path.exists():
            return
        with self.ready_path.open() as stream:
            value = json.load(stream)
        ids = value.get("coalitions") if isinstance(value, dict) else None
        pid = value.get("pid") if isinstance(value, dict) else None
        created = value.get("created") if isinstance(value, dict) else None
        if (
            not isinstance(ids, list)
            or len(ids) != 2
            or not all(isinstance(item, int) and item > 0 for item in ids)
            or not isinstance(pid, int)
            or pid <= 0
            or not isinstance(created, int | float)
        ):
            raise TypeError("Darwin target readiness is incomplete")
        coalitions = int(ids[0]), int(ids[1])
        supervisor_coalitions = self.proc.coalitions(os.getpid())
        if supervisor_coalitions is None:
            raise RuntimeError("libproc could not read the supervisor coalition")
        if coalitions[0] == supervisor_coalitions[0]:
            raise RuntimeError(
                "launchd did not isolate the Nix target resource coalition"
            )
        self.coalitions = coalitions
        self.leader = pid, float(created)

    def leader_running(self) -> bool:
        if self.leader is None:
            return False
        pid, created = self.leader
        try:
            process = psutil.Process(pid)
            return (
                process.create_time() == created
                and process.is_running()
                and process.status() != psutil.STATUS_ZOMBIE
            )
        except (psutil.NoSuchProcess, psutil.ZombieProcess):
            return False

    def terminal_status(self) -> int | None:
        status = self.read_status()
        if status is not None or self.leader is None or self.leader_running():
            return status
        # The wrapper publishes by atomic rename before exiting. Re-reading after
        # its stable identity is gone closes the publication versus exit race.
        status = self.read_status()
        if status is None:
            raise RuntimeError("launchd target exited without a status")
        return status

    def remember_members(
        self,
        members: dict[tuple[int, float], _Member],
    ) -> None:
        self.load_coalitions()
        if self.coalitions is None:
            return
        candidates: list[_Member] = []
        for pid in psutil.pids():
            if self.proc.coalitions(pid) != self.coalitions:
                continue
            try:
                candidates.append(_Member.capture(psutil.Process(pid)))
            except (ProcessLookupError, psutil.NoSuchProcess, psutil.ZombieProcess):
                continue
        for member in candidates:
            if self.proc.coalitions(member.process.pid) != self.coalitions:
                member.close()
                continue
            previous = members.setdefault(member.identity, member)
            if previous is not member:
                member.close()

    def task_count(self) -> int:
        self.load_coalitions()
        if self.coalitions is None:
            return 0
        return self.proc.task_count(self.coalitions[0])

    def remove(self) -> None:
        if self.removed or not self.bootstrap_attempted:
            return
        result = subprocess.run(
            ["/bin/launchctl", "bootout", self.target],
            check=False,
            capture_output=True,
            text=True,
            timeout=_DARWIN_LAUNCH_SECONDS,
        )
        self.removed = True
        if result.returncode != 0:
            detail = result.stderr.strip() or result.stdout.strip() or "no detail"
            raise RuntimeError(f"launchctl could not boot out {self.label}: {detail}")

    def close(self) -> None:
        for path in (
            self.config_path,
            self.ready_path,
            self.status_path,
            self.plist_path,
            self.stdout_path,
            self.stderr_path,
        ):
            with contextlib.suppress(FileNotFoundError):
                path.unlink()
            with contextlib.suppress(FileNotFoundError):
                _json_temporary(path).unlink()
        self.root.rmdir()


def _pidfd_exited(pidfd: int) -> bool:
    import select

    readable, _, _ = select.select([pidfd], [], [], 0)
    return bool(readable)


def _prepare_owner_fd(owner_fd: int) -> int:
    """Keep the lifetime pipe distinct from target stdio."""
    if owner_fd <= 2:
        raise ValueError("the nix supervisor owner fd must be above stderr")
    os.set_blocking(owner_fd, False)
    os.set_inheritable(owner_fd, False)  # noqa: FBT003 -- positional-only stdlib API
    return owner_fd


def _become_subreaper() -> None:
    """Make detached Linux descendants return to this supervisor."""
    if sys.platform != "linux":
        return
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(_PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))
    if _PIDFD_OPEN is None or _PIDFD_SEND_SIGNAL is None:
        raise RuntimeError("the nix supervisor requires pidfd support on Linux")


def _start_target(owner_fd: int, argv: list[str]) -> int:
    target = os.fork()
    if target != 0:
        return target
    try:
        os.close(owner_fd)
        os.setsid()
        os.execv(argv[0], argv)  # noqa: S606 -- structured argv, never a shell
    except BaseException as exc:
        message = (
            f"nix supervisor could not exec {argv[0]}: {type(exc).__name__}: {exc}\n"
        )
        with contextlib.suppress(OSError):
            os.write(2, message.encode(errors="replace"))
        os._exit(126 if isinstance(exc, PermissionError) else 127)


def _wait_for_child(pid: int) -> int:
    while True:
        try:
            waited, status = os.waitpid(pid, 0)
        except InterruptedError:
            continue
        if waited != pid:
            raise RuntimeError(f"waitpid returned {waited} for child {pid}")
        return status


def _run_darwin_target(config_path: Path) -> NoReturn:
    config: _DarwinTargetConfig | None = None
    target: int | None = None
    status: int | None = None
    try:
        config = _DarwinTargetConfig.read(config_path)
        coalitions = _DarwinProc.create().coalitions(os.getpid())
        if coalitions is None:
            raise RuntimeError("libproc could not read the launchd target coalition")
        process = psutil.Process()
        _write_private_json(
            config.ready,
            {
                "coalitions": list(coalitions),
                "created": process.create_time(),
                "pid": process.pid,
            },
        )
        target = os.fork()
        if target == 0:
            try:
                os.chdir(config.cwd)
                os.setsid()
                os.execve(  # noqa: S606 -- structured argv, never a shell
                    config.argv[0],
                    config.argv,
                    config.environment,
                )
            except BaseException as exc:
                message = (
                    f"nix supervisor could not exec {config.argv[0]}: "
                    f"{type(exc).__name__}: {exc}\n"
                )
                with contextlib.suppress(OSError):
                    os.write(2, message.encode(errors="replace"))
                os._exit(126 if isinstance(exc, PermissionError) else 127)
        status = _wait_for_child(target)
        target = None
        _write_private_json(config.status, {"status": status})
    except BaseException as exc:
        if target is not None and target > 0:
            with contextlib.suppress(ProcessLookupError):
                os.kill(target, signal.SIGKILL)
            with contextlib.suppress(ChildProcessError):
                _wait_for_child(target)
        message = f"nix launchd target failed: {type(exc).__name__}: {exc}\n"
        with contextlib.suppress(OSError):
            os.write(2, message.encode(errors="replace"))
        if config is not None and status is None:
            with contextlib.suppress(BaseException):
                _write_private_json(config.status, {"status": 125 << 8})
    os._exit(0)


def _remember_descendants(
    supervisor: psutil.Process,
    members: dict[tuple[int, float], _Member],
) -> None:
    try:
        descendants = supervisor.children(recursive=True)
    except (psutil.NoSuchProcess, psutil.ZombieProcess):
        return
    for process in descendants:
        try:
            member = _Member.capture(process)
        except (ProcessLookupError, psutil.NoSuchProcess, psutil.ZombieProcess):
            continue
        previous = members.setdefault(member.identity, member)
        if previous is not member:
            member.close()


def _reap_children(statuses: dict[int, int]) -> None:
    while True:
        try:
            pid, status = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if pid == 0:
            return
        statuses.setdefault(pid, status)


def _terminate_tree(
    supervisor: psutil.Process,
    members: dict[tuple[int, float], _Member],
    statuses: dict[int, int],
) -> None:
    deadline = time.monotonic() + _SHUTDOWN_SECONDS
    while True:
        _remember_descendants(supervisor, members)
        for member in reversed(tuple(members.values())):
            if member.running():
                member.kill()
        _reap_children(statuses)
        _remember_descendants(supervisor, members)
        if not any(member.running() for member in members.values()):
            _reap_children(statuses)
            if not supervisor.children():
                return
        if time.monotonic() >= deadline:
            alive = [
                str(member.process.pid)
                for member in members.values()
                if member.running()
            ]
            raise RuntimeError(
                "nix supervisor could not terminate process identities within "
                f"{_SHUTDOWN_SECONDS:g}s: {', '.join(alive) or 'unknown child'}"
            )
        time.sleep(_SHUTDOWN_POLL_SECONDS)


def _kill_relays(relays: list[int], statuses: dict[int, int]) -> None:
    for relay in relays:
        if relay in statuses:
            continue
        with contextlib.suppress(ProcessLookupError):
            os.kill(relay, signal.SIGKILL)
    for relay in relays:
        if relay in statuses:
            continue
        with contextlib.suppress(ChildProcessError):
            statuses[relay] = _wait_for_child(relay)


def _terminate_darwin_job(
    job: _DarwinJob,
    members: dict[tuple[int, float], _Member],
    statuses: dict[int, int],
) -> None:
    try:
        remove_error: BaseException | None = None
        try:
            job.remove()
        except BaseException as exc:
            remove_error = exc

        job.load_coalitions()
        if not job.loaded and job.coalitions is None:
            _kill_relays(job.relays, statuses)
            if remove_error is not None:
                raise remove_error
            return

        deadline = time.monotonic() + _SHUTDOWN_SECONDS
        while True:
            job.remember_members(members)
            for member in reversed(tuple(members.values())):
                if member.running():
                    member.kill()
            _reap_children(statuses)
            task_count = job.task_count()
            running_relays = [relay for relay in job.relays if relay not in statuses]
            if task_count == 0 and not running_relays:
                failed_relays = [
                    str(relay) for relay in job.relays if statuses.get(relay, 0) != 0
                ]
                if failed_relays:
                    raise RuntimeError(
                        "nix supervisor output relays failed: "
                        + ", ".join(failed_relays)
                    )
                if remove_error is not None:
                    raise remove_error
                return
            if time.monotonic() >= deadline:
                _kill_relays(job.relays, statuses)
                alive = [
                    str(member.process.pid)
                    for member in members.values()
                    if member.running()
                ]
                raise RuntimeError(
                    "nix supervisor could not empty launchd coalition "
                    f"{job.coalitions} within {_SHUTDOWN_SECONDS:g}s: "
                    f"{task_count} tasks, {', '.join(alive) or 'no visible member'}"
                )
            time.sleep(_SHUTDOWN_POLL_SECONDS)
    except BaseException as exc:
        for member in reversed(tuple(members.values())):
            try:
                if member.running():
                    member.kill()
            except BaseException as cleanup_error:
                exc.add_note(
                    "nix supervisor member cleanup also failed: "
                    f"{type(cleanup_error).__name__}: {cleanup_error}"
                )
        try:
            _kill_relays(job.relays, statuses)
        except BaseException as cleanup_error:
            exc.add_note(
                "nix supervisor relay cleanup also failed: "
                f"{type(cleanup_error).__name__}: {cleanup_error}"
            )
        raise


def _exit_code(status: int) -> int:
    code = os.waitstatus_to_exitcode(status)
    return code if code >= 0 else 128 - code


def main() -> NoReturn:
    if len(sys.argv) < 3:
        raise SystemExit("usage: _supervise.py OWNER_FD COMMAND [ARG ...]")

    owner_fd = _prepare_owner_fd(int(sys.argv[1]))
    argv = sys.argv[2:]
    _become_subreaper()
    supervisor = psutil.Process()
    members: dict[tuple[int, float], _Member] = {}
    statuses: dict[int, int] = {}
    selector = selectors.DefaultSelector()
    selector.register(owner_fd, selectors.EVENT_READ)
    owner_lost = False
    target: int | None = None
    target_status: int | None = None
    job: _DarwinJob | None = None
    failure: BaseException | None = None

    try:
        if sys.platform == "darwin":
            job = _DarwinJob.allocate(argv)
            job.start(owner_fd)
            ready_deadline = time.monotonic() + _DARWIN_LAUNCH_SECONDS
            while target_status is None and not owner_lost:
                _reap_children(statuses)
                job.load_coalitions()
                target_status = job.terminal_status()
                if target_status is not None:
                    break
                if job.coalitions is None and time.monotonic() >= ready_deadline:
                    raise RuntimeError("launchd target did not publish its coalition")
                if selector.select(_WATCH_SECONDS):
                    with contextlib.suppress(BlockingIOError):
                        owner_lost = not os.read(owner_fd, 1)
        else:
            target = _start_target(owner_fd, argv)
            while target not in statuses and not owner_lost:
                _reap_children(statuses)
                if target in statuses:
                    break
                if selector.select(_WATCH_SECONDS):
                    with contextlib.suppress(BlockingIOError):
                        owner_lost = not os.read(owner_fd, 1)
    except BaseException as exc:
        failure = exc
    finally:
        if job is not None:
            try:
                _terminate_darwin_job(job, members, statuses)
            except BaseException as cleanup_error:
                if failure is None:
                    failure = cleanup_error
                else:
                    failure.add_note(
                        "nix supervisor cleanup also failed: "
                        f"{type(cleanup_error).__name__}: {cleanup_error}"
                    )
            try:
                if target_status is None:
                    target_status = job.read_status()
                job.close()
            except BaseException as cleanup_error:
                if failure is None:
                    failure = cleanup_error
                else:
                    failure.add_note(
                        "nix supervisor launchd cleanup also failed: "
                        f"{type(cleanup_error).__name__}: {cleanup_error}"
                    )
        elif target is not None:
            try:
                _terminate_tree(supervisor, members, statuses)
            except BaseException as cleanup_error:
                if failure is None:
                    failure = cleanup_error
                else:
                    failure.add_note(
                        "nix supervisor cleanup also failed: "
                        f"{type(cleanup_error).__name__}: {cleanup_error}"
                    )
        selector.close()
        os.close(owner_fd)
        for member in members.values():
            member.close()

    if failure is not None:
        print(
            f"nix supervisor failed: {type(failure).__name__}: {failure}",
            file=sys.stderr,
            flush=True,
        )
        raise SystemExit(125) from failure

    status = target_status if job is not None else statuses.get(target)
    raise SystemExit(125 if status is None else _exit_code(status))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--darwin-target":
        if sys.platform != "darwin":
            raise SystemExit("the launchd target mode requires Darwin")
        _run_darwin_target(Path(sys.argv[2]))
    else:
        main()
