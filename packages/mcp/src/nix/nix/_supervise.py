"""Run one command beneath a lifetime-bound process supervisor."""

from __future__ import annotations

import contextlib
import ctypes
import fcntl
import os
import selectors
import signal
import sys
import time
from dataclasses import dataclass
from typing import NoReturn

import psutil

_WATCH_SECONDS = 0.1
_SHUTDOWN_POLL_SECONDS = 0.01
_SHUTDOWN_SECONDS = 5.0
_PR_SET_CHILD_SUBREAPER = 36
_PIDFD_OPEN = getattr(os, "pidfd_open", None)
_PIDFD_SEND_SIGNAL = getattr(signal, "pidfd_send_signal", None)


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


def _pidfd_exited(pidfd: int) -> bool:
    import select

    readable, _, _ = select.select([pidfd], [], [], 0)
    return bool(readable)


def _prepare_owner_fd(owner_fd: int) -> int:
    """Keep the lifetime pipe distinct from target stdio."""
    if owner_fd <= 2:
        replacement = fcntl.fcntl(owner_fd, fcntl.F_DUPFD_CLOEXEC, 3)
        os.close(owner_fd)
        owner_fd = replacement
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
        statuses[pid] = status


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
    target = _start_target(owner_fd, argv)
    members: dict[tuple[int, float], _Member] = {}
    statuses: dict[int, int] = {}
    selector = selectors.DefaultSelector()
    selector.register(owner_fd, selectors.EVENT_READ)
    owner_lost = False

    try:
        while target not in statuses and not owner_lost:
            if sys.platform != "linux":
                # Darwin has no subreaper. Capture descendants while their
                # parentage is still visible, then signal the same identities.
                _remember_descendants(supervisor, members)
            _reap_children(statuses)
            if target in statuses:
                break
            if selector.select(_WATCH_SECONDS):
                with contextlib.suppress(BlockingIOError):
                    owner_lost = not os.read(owner_fd, 1)
        _terminate_tree(supervisor, members, statuses)
    except BaseException as exc:
        print(
            f"nix supervisor failed: {type(exc).__name__}: {exc}",
            file=sys.stderr,
            flush=True,
        )
        raise SystemExit(125) from exc
    finally:
        selector.close()
        os.close(owner_fd)
        for member in members.values():
            member.close()

    status = statuses.get(target)
    raise SystemExit(125 if status is None else _exit_code(status))


if __name__ == "__main__":
    main()
