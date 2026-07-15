"""Exec one command with a watcher tied to the owning kernel's lifetime."""

from __future__ import annotations

import contextlib
import os
import signal
import sys
from typing import NoReturn


def _watch(owner_fd: int, process_group: int) -> NoReturn:
    null_fd = os.open(os.devnull, os.O_RDWR)
    for standard_fd in (0, 1, 2):
        os.dup2(null_fd, standard_fd)
    if null_fd > 2:
        os.close(null_fd)

    # The kernel retains the only write end. EOF therefore means that the job
    # released ownership or that the kernel disappeared without running cleanup.
    while os.read(owner_fd, 1):
        pass
    with contextlib.suppress(ProcessLookupError):
        os.killpg(process_group, signal.SIGKILL)
    os._exit(0)


def main() -> NoReturn:
    if len(sys.argv) < 3:
        raise SystemExit("usage: python -m nix._owner_exec OWNER_FD COMMAND [ARG ...]")

    owner_fd = int(sys.argv[1])
    argv = sys.argv[2:]
    process_group = os.getpgrp()
    watcher = os.fork()
    if watcher == 0:
        _watch(owner_fd, process_group)

    os.close(owner_fd)
    try:
        os.execvp(argv[0], argv)  # noqa: S606 -- structured argv is the launcher's boundary
    except OSError as exc:
        print(f"could not execute {argv[0]}: {exc}", file=sys.stderr, flush=True)
        raise SystemExit(127) from exc


if __name__ == "__main__":
    main()
