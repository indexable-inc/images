#!/usr/bin/env python3
from __future__ import annotations

import base64
import datetime as dt
import errno
import fcntl
import json
import os
import pty
import re
import select
import selectors
import shlex
import signal
import struct
import sys
import termios
import threading
import time
import tty
from collections import deque
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from types import FrameType


DEFAULT_HEAD_LINES = 80
DEFAULT_TAIL_LINES = 80
READ_SIZE = 65536
EXPECTED_PTY_ERRORS = {errno.EBADF, errno.EIO, errno.EPIPE}
EXPECTED_STDOUT_CLOSE_ERRORS = {errno.EBADF, errno.EPIPE}
RETRYABLE_WRITE_ERRORS = {errno.EAGAIN, errno.EWOULDBLOCK}
SignalHandler = signal.Handlers | int | Callable[[int, FrameType | None], None] | None


@dataclass(frozen=True)
class ArtifactPaths:
    session: Path
    output: Path
    typescript: Path
    timing: Path
    events: Path
    lines: Path
    cast: Path
    summary: Path
    replay: Path
    live: Path


@dataclass(frozen=True)
class Sample:
    monotonic_ns: int
    epoch_ns: int
    elapsed_ns: int
    timestamp: str


class JsonlWriter:
    def __init__(self, path: Path) -> None:
        self._file = path.open("a", encoding="utf-8", buffering=1)

    def write(self, value: dict[str, object] | list[object]) -> None:
        json.dump(value, self._file, ensure_ascii=False, separators=(",", ":"))
        self._file.write("\n")
        self._file.flush()

    def close(self) -> None:
        self._file.close()


class DisplayLimiter:
    def __init__(
        self,
        *,
        head_lines: int,
        tail_lines: int,
        print_mode: str,
        output_path: Path,
        stderr_fd: int,
        stdout_fd: int,
    ) -> None:
        self._head_lines = head_lines
        self._tail_lines = tail_lines
        self._print_mode = print_mode
        self._output_path = output_path
        self._stderr_fd = stderr_fd
        self._stdout_fd = stdout_fd
        self._tail: deque[bytes] = deque(maxlen=tail_lines)
        self._announced = False
        self._stdout_open = True

    def emit_line(self, line_no: int, data: bytes) -> None:
        if self._print_mode == "none":
            return
        if self._print_mode == "full" or line_no <= self._head_lines:
            self._write_stdout(data)
            return

        if self._tail_lines > 0:
            self._tail.append(data)
        if not self._announced:
            self._announced = True
            self._write_stderr(
                "\n"
                f"ix run: output exceeded {self._head_lines} line(s); "
                f"recording the full live stream at {self._output_path}\n"
            )

    def finish(self, total_lines: int) -> None:
        if (
            self._print_mode != "summary"
            or total_lines <= self._head_lines
            or self._tail_lines <= 0
            or not self._tail
        ):
            return

        omitted = max(0, total_lines - self._head_lines - len(self._tail))
        self._write_stderr(
            f"ix run: showing the last {len(self._tail)} line(s); "
            f"{omitted} middle line(s) omitted\n"
        )
        for line in self._tail:
            self._write_stdout(line)

    def _write_stdout(self, data: bytes) -> None:
        if not self._stdout_open:
            return
        try:
            os.write(self._stdout_fd, data)
        except OSError as exc:
            if exc.errno not in EXPECTED_STDOUT_CLOSE_ERRORS:
                raise
            self._stdout_open = False

    def _write_stderr(self, message: str) -> None:
        try:
            os.write(self._stderr_fd, message.encode())
        except OSError:
            return


class LineRecorder:
    def __init__(self, writer: JsonlWriter, display: DisplayLimiter) -> None:
        self._writer = writer
        self._display = display
        self._buffer = bytearray()
        self._started_elapsed_ns: int | None = None
        self._started_epoch_ns: int | None = None
        self._started_at: str | None = None
        self._previous_line_ended_elapsed_ns = 0
        self.line_count = 0

    def add(self, data: bytes, sample: Sample) -> None:
        pieces = data.split(b"\n")
        for index, piece in enumerate(pieces):
            if piece and self._started_elapsed_ns is None:
                self._mark_start(sample)
            self._buffer.extend(piece)
            if index < len(pieces) - 1:
                if self._started_elapsed_ns is None:
                    self._mark_start(sample)
                self._buffer.append(10)
                self._emit(sample, complete=True)

    def finish(self, sample: Sample) -> None:
        if self._buffer:
            if self._started_elapsed_ns is None:
                self._mark_start(sample)
            self._emit(sample, complete=False)
        self._display.finish(self.line_count)

    def _mark_start(self, sample: Sample) -> None:
        self._started_elapsed_ns = sample.elapsed_ns
        self._started_epoch_ns = sample.epoch_ns
        self._started_at = sample.timestamp

    def _emit(self, sample: Sample, *, complete: bool) -> None:
        self.line_count += 1
        raw = bytes(self._buffer)
        self._buffer.clear()
        text = raw.decode("utf-8", errors="replace")
        if text.endswith("\n"):
            text = text[:-1]
            if text.endswith("\r"):
                text = text[:-1]
        delta_since_previous_line_ns = sample.elapsed_ns - self._previous_line_ended_elapsed_ns
        self._writer.write(
            {
                "type": "line",
                "line_no": self.line_count,
                "started_at": self._started_at or sample.timestamp,
                "started_epoch_ns": self._started_epoch_ns or sample.epoch_ns,
                "started_elapsed_ns": self._started_elapsed_ns or sample.elapsed_ns,
                "ended_at": sample.timestamp,
                "ended_epoch_ns": sample.epoch_ns,
                "ended_elapsed_ns": sample.elapsed_ns,
                "delta_since_previous_line_ns": delta_since_previous_line_ns,
                "byte_count": len(raw),
                "complete": complete,
                "text": text,
            }
        )
        self._display.emit_line(self.line_count, raw)
        self._started_elapsed_ns = None
        self._started_epoch_ns = None
        self._started_at = None
        self._previous_line_ended_elapsed_ns = sample.elapsed_ns


class Recorder:
    def __init__(
        self,
        *,
        paths: ArtifactPaths,
        start_monotonic_ns: int,
        terminal: dict[str, object],
        display: DisplayLimiter,
    ) -> None:
        self._paths = paths
        self._start_monotonic_ns = start_monotonic_ns
        self._last_output_ns = start_monotonic_ns
        self._raw = paths.typescript.open("ab", buffering=0)
        self._output = paths.output.open("ab", buffering=0)
        self._timing = paths.timing.open("a", encoding="utf-8", buffering=1)
        self._events = JsonlWriter(paths.events)
        self._lines = JsonlWriter(paths.lines)
        self._cast = paths.cast.open("a", encoding="utf-8", buffering=1)
        self._line_recorder = LineRecorder(self._lines, display)
        self._seq = 0
        self.byte_count = 0
        self.chunk_count = 0
        self._write_cast_header(terminal)

    @property
    def line_count(self) -> int:
        return self._line_recorder.line_count

    def record(self, data: bytes, sample: Sample) -> None:
        self._raw.write(data)
        self._output.write(data)
        delay_ns = max(0, sample.monotonic_ns - self._last_output_ns)
        self._last_output_ns = sample.monotonic_ns
        self._timing.write(f"{delay_ns / 1_000_000_000:.6f} {len(data)}\n")
        text = data.decode("utf-8", errors="replace")
        self._events.write(
            {
                "type": "output",
                "seq": self._seq,
                "timestamp": sample.timestamp,
                "epoch_ns": sample.epoch_ns,
                "elapsed_ns": sample.elapsed_ns,
                "delay_ns": delay_ns,
                "stream": "pty",
                "byte_count": len(data),
                "line_feed_count": data.count(b"\n"),
                "text": text,
                "data_base64": base64.b64encode(data).decode("ascii"),
            }
        )
        self._cast.write(json.dumps([sample.elapsed_ns / 1_000_000_000, "o", text]) + "\n")
        self._cast.flush()
        self._line_recorder.add(data, sample)
        self._seq += 1
        self.byte_count += len(data)
        self.chunk_count += 1

    def finish(self, sample: Sample) -> None:
        self._line_recorder.finish(sample)

    def close(self) -> None:
        self._raw.close()
        self._output.close()
        self._timing.close()
        self._events.close()
        self._lines.close()
        self._cast.close()

    def _write_cast_header(self, terminal: dict[str, object]) -> None:
        header = {
            "version": 2,
            "width": terminal["columns"],
            "height": terminal["rows"],
            "timestamp": terminal["started_epoch_seconds"],
            "command": terminal["command"],
            "env": terminal["env"],
        }
        self._cast.write(json.dumps(header) + "\n")
        self._cast.flush()


def utc_now() -> dt.datetime:
    return dt.datetime.now(dt.UTC)


def isoformat(value: dt.datetime) -> str:
    return value.isoformat(timespec="microseconds").replace("+00:00", "Z")


def sample(start_monotonic_ns: int) -> Sample:
    monotonic_ns = time.monotonic_ns()
    now = utc_now()
    return Sample(
        monotonic_ns=monotonic_ns,
        epoch_ns=time.time_ns(),
        elapsed_ns=monotonic_ns - start_monotonic_ns,
        timestamp=isoformat(now),
    )


def env_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError as exc:
        raise SystemExit(f"{name} must be an integer, got {raw!r}") from exc
    if value < 0:
        raise SystemExit(f"{name} must be greater than or equal to zero")
    return value


def print_mode() -> str:
    mode = os.environ.get("IX_RUN_PRINT", "summary")
    valid = {"summary", "full", "none"}
    if mode not in valid:
        raise SystemExit(f"IX_RUN_PRINT must be one of: {', '.join(sorted(valid))}")
    return mode


def state_root() -> Path:
    raw = os.environ.get("IX_RUN_DIR")
    if raw:
        return Path(raw).expanduser().resolve()
    return (Path.cwd() / ".ix" / "run").resolve()


def slug_for(argv: list[str]) -> str:
    basename = Path(argv[0]).name or "command"
    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", basename).strip("-._")
    return (slug or "command")[:32]


def session_paths(argv: list[str], started: dt.datetime) -> ArtifactPaths:
    root = state_root()
    root.mkdir(parents=True, exist_ok=True)
    stamp = started.strftime("%Y%m%dT%H%M%SZ")
    base = f"{stamp}-{slug_for(argv)}-{os.getpid()}"
    session = root / base
    suffix = 1
    while session.exists():
        suffix += 1
        session = root / f"{base}-{suffix}"
    session.mkdir(mode=0o700)

    latest = root / "latest"
    try:
        if latest.is_symlink() or latest.is_file():
            latest.unlink()
        latest.symlink_to(session, target_is_directory=True)
    except OSError as exc:
        write_stderr(f"ix run: warning: could not update latest symlink {latest}: {exc}\n")

    return ArtifactPaths(
        session=session,
        output=session / "output.log",
        typescript=session / "typescript",
        timing=session / "timing.log",
        events=session / "events.jsonl",
        lines=session / "lines.jsonl",
        cast=session / "session.cast",
        summary=session / "summary.json",
        replay=session / "replay",
        live=session / "live",
    )


def write_json(path: Path, data: dict[str, object]) -> None:
    tmp = path.with_suffix(f"{path.suffix}.tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    tmp.replace(path)


def shell_script(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8")
    path.chmod(0o755)


def replay_command(paths: ArtifactPaths) -> list[str]:
    scriptreplay = os.environ.get("IX_RUN_SCRIPTREPLAY", "scriptreplay")
    return [
        scriptreplay,
        "--timing",
        str(paths.timing),
        "--log-out",
        str(paths.typescript),
        "--divisor",
        "1",
    ]


def write_helper_scripts(paths: ArtifactPaths) -> None:
    scriptreplay = os.environ.get("IX_RUN_SCRIPTREPLAY", "scriptreplay")
    shell_script(
        paths.replay,
        "\n".join(
            [
                "#!/bin/sh",
                "set -eu",
                'divisor="${1:-1}"',
                "exec "
                + " ".join(
                    [
                        shlex.quote(scriptreplay),
                        "--timing",
                        shlex.quote(str(paths.timing)),
                        "--log-out",
                        shlex.quote(str(paths.typescript)),
                        "--divisor",
                        '"$divisor"',
                    ]
                ),
                "",
            ]
        ),
    )
    shell_script(
        paths.live,
        "\n".join(
            [
                "#!/bin/sh",
                "set -eu",
                "exec tail -n +1 -f " + shlex.quote(str(paths.output)),
                "",
            ]
        ),
    )


def terminal_size() -> tuple[int, int]:
    stdout = getattr(sys, "stdout", None)
    if stdout is not None:
        try:
            if stdout.isatty():
                size = os.get_terminal_size(stdout.fileno())
                return size.columns, size.lines
        except (AttributeError, OSError, ValueError):
            size = os.get_terminal_size()
            return size.columns, size.lines
    size = os.get_terminal_size()
    return size.columns, size.lines


def safe_terminal_size() -> tuple[int, int]:
    try:
        return terminal_size()
    except (AttributeError, OSError, ValueError):
        fallback = os.environ.get("COLUMNS"), os.environ.get("LINES")
        try:
            columns = int(fallback[0] or "80")
            rows = int(fallback[1] or "24")
        except ValueError:
            return 80, 24
        return max(columns, 1), max(rows, 1)


def set_winsize(fd: int, columns: int, rows: int) -> None:
    packed = struct.pack("HHHH", rows, columns, 0, 0)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, packed)


def spawn(argv: list[str], *, columns: int, rows: int) -> tuple[int, int]:
    master_fd, slave_fd = pty.openpty()
    set_winsize(slave_fd, columns, rows)
    pid = os.fork()
    if pid == 0:
        try:
            os.setsid()
            fcntl.ioctl(slave_fd, termios.TIOCSCTTY, 0)
            os.dup2(slave_fd, 0)
            os.dup2(slave_fd, 1)
            os.dup2(slave_fd, 2)
            os.close(master_fd)
            if slave_fd > 2:
                os.close(slave_fd)
            os.execvp(argv[0], argv)
        except FileNotFoundError:
            os.write(2, f"run: command not found: {argv[0]}\n".encode())
            os._exit(127)
        except OSError as exc:
            os.write(2, f"run: could not execute {argv[0]}: {exc}\n".encode())
            os._exit(126)
    os.close(slave_fd)
    return pid, master_fd


def set_nonblocking(fd: int) -> int:
    flags = fcntl.fcntl(fd, fcntl.F_GETFL)
    fcntl.fcntl(fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)
    return flags


def optional_stdin() -> tuple[int | None, bool]:
    stdin = getattr(sys, "stdin", None)
    if stdin is None:
        return None, False
    try:
        return stdin.fileno(), stdin.isatty()
    except (AttributeError, OSError, ValueError):
        return None, False


def stdio_fd(name: str, fallback: int) -> int:
    stream = getattr(sys, name, None)
    if stream is None:
        return fallback
    try:
        return stream.fileno()
    except (AttributeError, OSError, ValueError):
        return fallback


def wait_master_writable(master_fd: int) -> bool:
    try:
        select.select([], [master_fd], [])
        return True
    except OSError as exc:
        if exc.errno in EXPECTED_PTY_ERRORS:
            return False
        raise
    except ValueError:
        return False


def wait_fd_readable(fd: int) -> bool:
    try:
        select.select([fd], [], [])
        return True
    except OSError as exc:
        if exc.errno in EXPECTED_PTY_ERRORS:
            return False
        raise
    except ValueError:
        return False


def write_master(master_fd: int, data: bytes) -> bool:
    remaining = memoryview(data)
    while remaining:
        try:
            written = os.write(master_fd, remaining)
        except BlockingIOError:
            if not wait_master_writable(master_fd):
                return False
            continue
        except InterruptedError:
            continue
        except OSError as exc:
            if exc.errno in EXPECTED_PTY_ERRORS:
                return False
            if exc.errno in RETRYABLE_WRITE_ERRORS:
                if not wait_master_writable(master_fd):
                    return False
                continue
            raise

        if written == 0:
            if not wait_master_writable(master_fd):
                return False
            continue
        remaining = remaining[written:]

    return True


def send_eot(master_fd: int) -> None:
    # EOT is best-effort: the child may already have closed the PTY.
    write_master(master_fd, b"\x04")


def forward_stdin(stdin_fd: int, master_fd: int) -> None:
    while True:
        try:
            data = os.read(stdin_fd, READ_SIZE)
        except BlockingIOError:
            if wait_fd_readable(stdin_fd):
                continue
            send_eot(master_fd)
            return
        except InterruptedError:
            continue
        except OSError as exc:
            if exc.errno in {errno.EBADF, errno.EIO, errno.EINVAL}:
                send_eot(master_fd)
                return
            if exc.errno in RETRYABLE_WRITE_ERRORS:
                if wait_fd_readable(stdin_fd):
                    continue
                send_eot(master_fd)
                return
            write_stderr(f"ix run: warning: stopped forwarding stdin: {exc}\n")
            send_eot(master_fd)
            return

        if not data:
            send_eot(master_fd)
            return
        if not write_master(master_fd, data):
            return


def status_code(status: int) -> tuple[str, int, int | None]:
    if os.WIFEXITED(status):
        code = os.WEXITSTATUS(status)
        return "exited", code, None
    if os.WIFSIGNALED(status):
        signum = os.WTERMSIG(status)
        return "signaled", 128 + signum, signum
    return "unknown", 1, None


def artifact_summary(paths: ArtifactPaths) -> dict[str, str]:
    return {
        "session": str(paths.session),
        "output": str(paths.output),
        "typescript": str(paths.typescript),
        "timing": str(paths.timing),
        "events": str(paths.events),
        "lines": str(paths.lines),
        "cast": str(paths.cast),
        "summary": str(paths.summary),
        "replay": str(paths.replay),
        "live": str(paths.live),
    }


def usage() -> str:
    return "\n".join(
        [
            "usage: run <command> [arg ...]",
            "",
            "Records a PTY session while running the command exactly as argv.",
            "",
            "environment:",
            f"  IX_RUN_HEAD_LINES  first lines to print, default {DEFAULT_HEAD_LINES}",
            f"  IX_RUN_TAIL_LINES  last lines to print, default {DEFAULT_TAIL_LINES}",
            "  IX_RUN_PRINT       summary, full, or none; default summary",
            "  IX_RUN_DIR         session directory root; default ./.ix/run",
        ]
    )


def write_stderr(message: str) -> None:
    try:
        os.write(stdio_fd("stderr", 2), message.encode())
    except OSError:
        return


def command_label(argv: list[str]) -> str:
    return " ".join(shlex.quote(arg) for arg in argv)


def terminal_environment() -> dict[str, str]:
    return {
        key: value
        for key in ["TERM", "COLORTERM", "SHELL"]
        if (value := os.environ.get(key)) is not None
    }


def initial_summary(
    *,
    argv: list[str],
    paths: ArtifactPaths,
    started: dt.datetime,
    start_epoch_ns: int,
    columns: int,
    rows: int,
    head_lines: int,
    tail_lines: int,
    mode: str,
    terminal_env: dict[str, str],
) -> dict[str, object]:
    replay = replay_command(paths)
    return {
        "schema_version": 1,
        "status": "running",
        "command": argv,
        "command_string": command_label(argv),
        "cwd": str(Path.cwd()),
        "pid": os.getpid(),
        "started_at": isoformat(started),
        "started_epoch_ns": start_epoch_ns,
        "terminal": {
            "columns": columns,
            "rows": rows,
            "env": terminal_env,
        },
        "limits": {
            "head_lines": head_lines,
            "tail_lines": tail_lines,
            "print_mode": mode,
        },
        "artifacts": artifact_summary(paths),
        "replay": {
            "scriptreplay": replay,
            "one_x": str(paths.replay),
            "two_x": f"{paths.replay} 2",
            "three_x": f"{paths.replay} 3",
        },
    }


def run(argv: list[str]) -> int:
    started = utc_now()
    start_epoch_ns = time.time_ns()
    start_monotonic_ns = time.monotonic_ns()
    head_lines = env_int("IX_RUN_HEAD_LINES", DEFAULT_HEAD_LINES)
    tail_lines = env_int("IX_RUN_TAIL_LINES", DEFAULT_TAIL_LINES)
    mode = print_mode()
    columns, rows = safe_terminal_size()
    paths = session_paths(argv, started)
    write_helper_scripts(paths)
    terminal_env = terminal_environment()

    summary = initial_summary(
        argv=argv,
        paths=paths,
        started=started,
        start_epoch_ns=start_epoch_ns,
        columns=columns,
        rows=rows,
        head_lines=head_lines,
        tail_lines=tail_lines,
        mode=mode,
        terminal_env=terminal_env,
    )
    write_json(paths.summary, summary)

    write_stderr(
        "\n".join(
            [
                f"ix run: recording {command_label(argv)}",
                f"ix run: session {paths.session}",
                f"ix run: live output {paths.output}",
                f"ix run: replay {paths.replay} [speed-divisor]",
                "",
            ]
        )
    )

    display = DisplayLimiter(
        head_lines=head_lines,
        tail_lines=tail_lines,
        print_mode=mode,
        output_path=paths.output,
        stderr_fd=stdio_fd("stderr", 2),
        stdout_fd=stdio_fd("stdout", 1),
    )
    terminal: dict[str, object] = {
        "columns": columns,
        "rows": rows,
        "started_epoch_seconds": int(started.timestamp()),
        "command": command_label(argv),
        "env": terminal_env,
    }
    recorder = Recorder(
        paths=paths,
        start_monotonic_ns=start_monotonic_ns,
        terminal=terminal,
        display=display,
    )
    pid, master_fd = spawn(argv, columns=columns, rows=rows)
    selector = selectors.DefaultSelector()
    selector.register(master_fd, selectors.EVENT_READ, "pty")
    set_nonblocking(master_fd)

    stdin_fd, stdin_is_tty = optional_stdin()
    stdin_attrs: list[int | list[bytes | int]] | None = None
    if stdin_fd is not None and stdin_is_tty:
        try:
            stdin_attrs = termios.tcgetattr(stdin_fd)
            tty.setraw(stdin_fd)
        except OSError as exc:
            write_stderr(f"ix run: warning: could not put stdin in raw mode: {exc}\n")

    if stdin_fd is None:
        send_eot(master_fd)
    else:
        threading.Thread(
            target=forward_stdin,
            args=(stdin_fd, master_fd),
            daemon=True,
            name="ix-run-stdin",
        ).start()

    child_status: int | None = None
    pty_open = True
    previous_handlers: dict[int, SignalHandler] = {}

    def resize(_signum: int, _frame: FrameType | None) -> None:
        new_columns, new_rows = safe_terminal_size()
        try:
            set_winsize(master_fd, new_columns, new_rows)
            os.killpg(pid, signal.SIGWINCH)
        except OSError:
            return

    def forward(signum: int, _frame: FrameType | None) -> None:
        try:
            os.killpg(pid, signum)
        except OSError:
            return

    previous_winch = signal.signal(signal.SIGWINCH, resize)
    for signum in [signal.SIGINT, signal.SIGTERM, signal.SIGHUP]:
        previous_handlers[signum] = signal.signal(signum, forward)

    try:
        while pty_open or child_status is None:
            for key, _events in selector.select(timeout=0.1):
                if key.data == "pty":
                    while True:
                        try:
                            data = os.read(master_fd, READ_SIZE)
                        except BlockingIOError:
                            break
                        except OSError as exc:
                            if exc.errno != errno.EIO:
                                raise
                            selector.unregister(master_fd)
                            pty_open = False
                            break
                        if not data:
                            selector.unregister(master_fd)
                            pty_open = False
                            break
                        recorder.record(data, sample(start_monotonic_ns))
            if child_status is None:
                try:
                    waited_pid, status = os.waitpid(pid, os.WNOHANG)
                except ChildProcessError:
                    waited_pid = pid
                    status = 1
                if waited_pid == pid:
                    child_status = status

            if child_status is not None and not pty_open:
                break

        if child_status is None:
            _waited_pid, child_status = os.waitpid(pid, 0)
    finally:
        signal.signal(signal.SIGWINCH, previous_winch)
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)
        if stdin_attrs is not None and stdin_fd is not None:
            termios.tcsetattr(stdin_fd, termios.TCSADRAIN, stdin_attrs)
        selector.close()
        try:
            os.close(master_fd)
        except OSError as exc:
            if exc.errno not in EXPECTED_PTY_ERRORS:
                raise

    finished_sample = sample(start_monotonic_ns)
    recorder.finish(finished_sample)
    state, exit_code, signum = status_code(child_status)
    duration_ns = finished_sample.elapsed_ns
    summary.update(
        {
            "status": state,
            "exit_code": exit_code,
            "signal": signum,
            "ended_at": finished_sample.timestamp,
            "ended_epoch_ns": finished_sample.epoch_ns,
            "duration_ns": duration_ns,
            "duration_seconds": duration_ns / 1_000_000_000,
            "output": {
                "byte_count": recorder.byte_count,
                "line_count": recorder.line_count,
                "chunk_count": recorder.chunk_count,
            },
        }
    )
    write_json(paths.summary, summary)
    recorder.close()
    write_stderr(
        f"ix run: {state} with code {exit_code} after {duration_ns / 1_000_000_000:.3f}s\n"
        f"ix run: structured events {paths.events}\n"
        f"ix run: structured lines {paths.lines}\n"
    )
    return exit_code


def main() -> int:
    argv = sys.argv[1:]
    if not argv:
        print(usage(), file=sys.stderr)
        return 2
    if argv[0] in {"-h", "--help"}:
        print(usage())
        return 0
    return run(argv)


if __name__ == "__main__":
    raise SystemExit(main())
