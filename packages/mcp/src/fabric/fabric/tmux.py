"""Agent CLI processes re-homed into tmux windows (index#3478).

``fabric.claude.session`` drives the ``claude`` CLI over stdio pipes
(stream-json), which is headless: nothing for a human to watch or kill.
This module is the one tmux seam for every agent process the orchestrator
spawns: the SDK still owns the pipe protocol end to end, but the CLI
process itself runs inside a window of the shared ``ix-agents`` tmux
session, so ``tmux attach -t ix-agents`` shows one live window per agent.

On by default: ``IX_FABRIC_TMUX`` unset or truthy means on; ``0`` /
``false`` / ``no`` / ``off`` turns it off; ``claude.session(..., tmux=...)``
overrides per call. When on, a missing ``tmux`` or ``claude`` binary fails
the spawn loudly rather than silently going headless.

Mechanics: the SDK's ``cli_path`` points at a generated shim script
(:func:`shim_path`, running :func:`shim_main`). The shim bridges the SDK's
pipes to two FIFOs, then opens a tmux window running
``python -m fabric.tmux <spec.json>`` (:func:`pane_main`). The pane process
spawns the real CLI with the shim's full environment (a tmux pane otherwise
inherits the tmux *server's* environment, losing the SDK's env), feeds it
the stdin FIFO, mirrors its raw stdout to the stdout FIFO (the SDK's
transport, byte-exact), renders that stream-json as a live human-readable
transcript in the pane (:mod:`fabric.render`, index#3496), and records the
exit code for the shim to exit with.
"""

from __future__ import annotations

import io
import json
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
from pathlib import Path
from types import FrameType

from . import render

__all__ = [
    "ENV_CLI",
    "ENV_KNOB",
    "ENV_WINDOW",
    "SESSION",
    "enabled",
    "pane_main",
    "real_cli",
    "shim_main",
    "shim_path",
    "window_name",
]

# The shared tmux session every agent window lives under.
SESSION = "ix-agents"

# The spawn-in-tmux knob, read by `enabled()`.
ENV_KNOB = "IX_FABRIC_TMUX"

# Shim inputs, set on the SDK subprocess env by `fabric.claude._sdk_client`:
# the resolved real CLI to run, and the tmux window name for this task.
ENV_CLI = "IX_FABRIC_TMUX_CLI"
ENV_WINDOW = "IX_FABRIC_TMUX_WINDOW"

# How long the shim waits for the pane process to open its end of the
# FIFOs before declaring the window dead on arrival.
CONNECT_TIMEOUT_S = 30.0

_FALSE = frozenset({"0", "false", "no", "off"})


def enabled() -> bool:
    """The spawn-in-tmux default: ``IX_FABRIC_TMUX``, on unless set falsy."""

    return os.environ.get(ENV_KNOB, "").strip().lower() not in _FALSE


def window_name(task: str) -> str:
    """A tmux-safe window name for a weave task id (``task:8hex``)."""

    return task.replace(":", "-").replace(".", "-")


def real_cli() -> str:
    """Resolve the real ``claude`` binary the shim will run in the pane."""

    path = shutil.which("claude")
    if path is None:
        raise FileNotFoundError(
            "fabric.tmux: `claude` not on PATH; the tmux shim needs the real CLI "
            f"(set {ENV_KNOB}=0 or pass tmux=False to run headless)"
        )
    return path


_shim_path: str | None = None


def shim_path() -> str:
    """The executable the SDK spawns as ``cli_path`` (cached per process).

    A two-line script on this interpreter, so the shim runs with this exact
    environment (fabric importable) no matter what PATH the SDK inherits.
    """

    global _shim_path
    if _shim_path is None or not Path(_shim_path).exists():
        fd, path = tempfile.mkstemp(prefix="ix-fabric-tmux-shim-", suffix=".py")
        with os.fdopen(fd, "w") as handle:
            handle.write(f"#!{sys.executable}\nfrom fabric.tmux import shim_main\nshim_main()\n")
        Path(path).chmod(0o755)
        _shim_path = path
    return _shim_path


def _tmux_binary() -> str:
    path = shutil.which("tmux")
    if path is None:
        raise FileNotFoundError(
            "fabric.tmux: `tmux` not on PATH but spawn-in-tmux is on "
            f"(set {ENV_KNOB}=0 or pass tmux=False to run headless)"
        )
    return path


def _new_window(name: str, command: str) -> tuple[str, str]:
    """Open a detached window in :data:`SESSION`; return (tmux binary, pane id)."""

    tmux = _tmux_binary()
    have = subprocess.run(
        [tmux, "has-session", "-t", f"={SESSION}"], capture_output=True, check=False
    )
    if have.returncode != 0:
        created = subprocess.run(
            [tmux, "new-session", "-d", "-s", SESSION],
            capture_output=True,
            text=True,
            check=False,
        )
        # Concurrent shims race to create the session; losing to a sibling
        # that just created it is success, anything else is not.
        if created.returncode != 0 and "duplicate session" not in created.stderr:
            raise RuntimeError(f"fabric.tmux: new-session failed: {created.stderr.strip()}")
    made = subprocess.run(
        [tmux, "new-window", "-d", "-P", "-F", "#{pane_id}", "-t", f"={SESSION}:", "-n", name, command],
        capture_output=True,
        text=True,
        check=False,
    )
    if made.returncode != 0:
        raise RuntimeError(f"fabric.tmux: new-window failed: {made.stderr.strip()}")
    return tmux, made.stdout.strip()


def shim_main() -> None:
    """SDK-facing side: bridge the SDK's stdio pipes to the tmux pane.

    Runs as its own process (the SDK's ``cli_path`` subprocess), so blocking
    IO here never touches the kernel's event loop.
    """

    args = sys.argv[1:]
    real = os.environ.get(ENV_CLI)
    if real is None:
        raise SystemExit(f"fabric.tmux shim: {ENV_CLI} is not set")
    if args == ["-v"]:
        # The SDK's startup version probe: answer directly, no window.
        os.execv(real, [real, *args])  # noqa: S606 -- exec the resolved CLI binary, argv is never shell-parsed
    window = os.environ.get(ENV_WINDOW, "agent")

    workdir = Path(tempfile.mkdtemp(prefix="ix-fabric-tmux-"))
    fifo_in = workdir / "in"
    fifo_out = workdir / "out"
    rc_path = workdir / "rc"
    os.mkfifo(fifo_in)
    os.mkfifo(fifo_out)
    spec_path = workdir / "spec.json"
    spec_path.write_text(
        json.dumps(
            {
                "argv": [real, *args],
                "env": dict(os.environ),
                "cwd": str(Path.cwd()),
                "stdin": str(fifo_in),
                "stdout": str(fifo_out),
                "rc": str(rc_path),
            }
        )
    )
    # `exec` so kill-pane HUPs the pane python directly, not a wrapper shell.
    # The pane inherits the tmux *server's* environment, so the shim's own
    # PYTHONPATH rides along explicitly: the pane's `import fabric` must
    # resolve the same module tree as the shim (the spec env only covers the
    # CLI child, which spawns after that import).
    pythonpath = os.environ.get("PYTHONPATH")
    argv = [sys.executable, "-m", "fabric.tmux", str(spec_path)]
    if pythonpath is not None:
        argv = ["env", f"PYTHONPATH={pythonpath}", *argv]
    command = "exec " + shlex.join(argv)
    tmux, pane = _new_window(window, command)

    def _on_terminate(signum: int, _frame: FrameType | None) -> None:
        subprocess.run([tmux, "kill-pane", "-t", pane], capture_output=True, check=False)
        os._exit(128 + signum)

    signal.signal(signal.SIGTERM, _on_terminate)
    signal.signal(signal.SIGINT, _on_terminate)

    paired = threading.Event()

    def _pump_stdin() -> None:
        # Blocks until the pane opens the FIFO's read end, then copies the
        # SDK's stdin through; closing on EOF is the CLI's stdin EOF.
        with fifo_in.open("wb") as sink:
            paired.set()
            stdin = sys.stdin.buffer
            # typeshed widens sys.stdin.buffer to BinaryIO, which lacks read1
            # (read would block for a full buffer).
            assert isinstance(stdin, io.BufferedReader), type(stdin)
            while chunk := stdin.read1(65536):
                sink.write(chunk)
                sink.flush()

    threading.Thread(target=_pump_stdin, name="stdin-pump", daemon=True).start()
    if not paired.wait(CONNECT_TIMEOUT_S):
        subprocess.run([tmux, "kill-pane", "-t", pane], capture_output=True, check=False)
        raise SystemExit(
            f"fabric.tmux shim: pane never came up within {CONNECT_TIMEOUT_S:.0f}s "
            f"(window {window!r}; inspect with `tmux attach -t {SESSION}`)"
        )

    try:
        with fifo_out.open("rb") as source:
            while chunk := source.read1(65536):
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
        # Pane killed before recording an exit code reads as failure, loudly.
        code = int(rc_path.read_text()) if rc_path.exists() else 1
    finally:
        shutil.rmtree(workdir, ignore_errors=True)
    raise SystemExit(code)


def _display(transcript: render.Transcript, line: bytes) -> None:
    """Show one CLI stdout line in the pane, rendered.

    The mirror FIFO is the SDK's transport, so a display bug must never
    sever the session: a rendering failure prints loudly and falls back to
    the raw line instead of raising out of the mirror loop.
    """

    try:
        text = transcript.feed(line)
    except Exception as exc:  # viewer only; see docstring
        text = f"fabric.render failed ({exc!r}) on: {line[:200]!r}"
    if text is not None:
        sys.stdout.write(text + "\n")
        sys.stdout.flush()


def pane_main(spec_path: str) -> None:
    """Pane-facing side: run the real CLI, mirror raw stdout to the SDK, render it in the pane."""

    spec = json.loads(Path(spec_path).read_text())

    def record(code: int) -> int:
        Path(spec["rc"]).write_text(str(code))
        return code

    code = 1
    try:
        with Path(spec["stdin"]).open("rb") as child_in, Path(spec["stdout"]).open("wb") as mirror:
            try:
                proc = subprocess.Popen(
                    spec["argv"],
                    stdin=child_in,
                    stdout=subprocess.PIPE,
                    env=spec["env"],
                    cwd=spec["cwd"],
                )
            except OSError as exc:
                print(f"fabric.tmux pane: spawn failed: {exc}", file=sys.stderr)
                code = record(127)
            else:
                stdout = proc.stdout
                # PIPE with default bufsize is buffered; typeshed widens to
                # IO[bytes], which lacks read1.
                assert isinstance(stdout, io.BufferedReader), type(stdout)
                transcript = render.Transcript()
                pending = b""
                while chunk := stdout.read1(65536):
                    mirror.write(chunk)
                    mirror.flush()
                    pending += chunk
                    while (end := pending.find(b"\n")) != -1:
                        line, pending = pending[:end], pending[end + 1 :]
                        _display(transcript, line)
                if pending:
                    _display(transcript, pending)
                # Record BEFORE the mirror FIFO closes: its EOF is the shim's
                # cue to read the exit code, so the file must exist by then.
                code = record(proc.wait())
    except BaseException:
        record(code)
        raise
    raise SystemExit(code)


if __name__ == "__main__":
    pane_main(sys.argv[1])
