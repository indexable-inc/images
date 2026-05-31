"""Native macOS VM control for the ix-mcp interpreter.

Bundled into the pinned interpreter the same way ``tui``, ``search`` and
``screen`` are, so every session can ``import macvm`` with no setup. Where
``screen`` captures the host desktop, ``macvm`` boots a guest VM and captures
*its* display, fully off-screen: nothing appears on the host desktop and the
host cursor is never touched. This is the way to verify on-screen rendering (a
GUI app, a boot screen) inside an isolated VM without taking over the machine.

    import macvm
    print(macvm.info())                       # is virtualization available?
    img = macvm.screenshot("/path/to/guest")  # boot the guest, return a PIL.Image
    img                                        # auto-rendered inline by python_eval/exec

A guest is a *bundle* directory (``disk.img``, ``aux.img``,
``hardware-model.bin``, ``machine-id.bin``) created once with
:func:`install`::

    macvm.install("/path/to/UniversalMac_26.5_Restore.ipsw", "/path/to/guest")

To *drive* a guest (synthetic keyboard/mouse plus on-demand screenshots), open a
:class:`Driver` as a context manager. It spawns the binary's ``drive-macos``
mode and talks to it in lockstep: every command returns the binary's one-line
acknowledgement, so a controller can capture a frame, locate a control in it
(with any host-side image tooling), click it, and capture again::

    with macvm.Driver("/path/to/guest") as d:
        d.click(0.5, 0.5)          # left-click at the centre of the display
        d.type_("hello")           # type printable ASCII
        d.key("return")            # press a named key
        d.press_down("cmd"); d.key("space"); d.release("cmd")  # a chord (Spotlight)
        img = d.shot()             # screenshot the framebuffer as a PIL.Image

:func:`drive` is a one-shot convenience that opens a :class:`Driver`, sends a
list of command strings, and returns the acks. :func:`screenshot_many` boots
several bundles concurrently and returns one image per bundle.

A host directory can be shared into the guest over virtio-fs with the ``shares``
argument on :func:`screenshot`, :class:`Driver`, :func:`drive`, and
:func:`screenshot_many`: a list of ``"TAG=HOSTDIR"`` specs. Tag ``auto`` uses
the macOS automount tag, mounting at ``/Volumes/My Shared Files``. This is how a
GUI app on the host is run inside the guest: share its directory in and launch
it from the share.

Each guest is an independent ``macos-vm`` process, so :class:`Driver` instances
and :func:`screenshot` calls are independent and safe to run in parallel; fan
out across several guests with :func:`screenshot_many` or by opening multiple
:class:`Driver` instances from separate threads.

The work is done by the ``macos-vm`` binary (a thin Rust binding over Apple's
Virtualization.framework). It holds the ``com.apple.security.virtualization``
entitlement by self-signing into a per-user cache on first use, so no manual
``codesign`` is needed. The capture reads the guest framebuffer IOSurface
directly, so it needs no Screen-Recording permission.

This module is macOS-only: it raises on a non-Darwin platform, and the
``macos-vm`` binary is only bundled into the interpreter on Darwin.
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterable, Sequence

    from PIL import Image

__all__ = [
    "Driver",
    "MacVmError",
    "drive",
    "info",
    "install",
    "screenshot",
    "screenshot_many",
]


class MacVmError(RuntimeError):
    """A macos-vm invocation failed, or the platform/binary is unavailable."""


def _binary() -> str:
    if sys.platform != "darwin":
        raise MacVmError("macvm is macOS-only")
    path = os.environ.get("IX_MACVM_BIN")
    if not path:
        raise MacVmError(
            "IX_MACVM_BIN is not set; the macos-vm binary is bundled into ix-mcp "
            "on Darwin only"
        )
    return path


def _share_args(shares: Iterable[str] | None) -> list[str]:
    """Expand ``"TAG=HOSTDIR"`` specs into repeated ``--share`` arguments."""
    if not shares:
        return []
    args: list[str] = []
    for spec in shares:
        args += ["--share", str(spec)]
    return args


def info() -> str:
    """Return whether Virtualization.framework can run a VM on this host."""
    out = subprocess.run(
        [_binary(), "info"], capture_output=True, text=True, check=False
    )
    return (out.stdout or out.stderr).strip()


def install(ipsw: str | os.PathLike, bundle: str | os.PathLike, disk_gib: int = 64, timeout: float = 2400) -> None:
    """Install macOS into a fresh ``bundle`` directory from a local ``ipsw``.

    Takes ~15-20 minutes. Raises :class:`MacVmError` on failure.
    """
    try:
        result = subprocess.run(
            [_binary(), "install-macos", "--ipsw", str(ipsw), "--bundle", str(bundle), "--disk-gib", str(disk_gib)],
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise MacVmError(f"install-macos timed out after {timeout}s") from exc
    if result.returncode != 0:
        raise MacVmError(f"install-macos failed: {result.stderr.strip()}")


def screenshot(
    bundle: str | os.PathLike,
    seconds: int = 20,
    timeout: float | None = None,
    shares: Sequence[str] | None = None,
) -> "Image.Image":
    """Boot the macOS guest in ``bundle`` off-screen and return a ``PIL.Image``
    of its display after ``seconds`` (the last frame captured).

    ``shares`` is a list of ``"TAG=HOSTDIR"`` virtio-fs specs (see the
    module docstring). Raises :class:`MacVmError` if the binary fails, times out,
    or produces no frame.
    """
    from PIL import Image

    bin_path = _binary()
    deadline = timeout if timeout is not None else seconds + 120
    with tempfile.TemporaryDirectory(prefix="ix-macvm-") as tmp:
        prefix = pathlib.Path(tmp) / "shot"
        try:
            result = subprocess.run(
                [bin_path, "boot-macos", "--bundle", str(bundle), "--out-prefix", str(prefix), "--seconds", str(seconds), *_share_args(shares)],
                capture_output=True,
                text=True,
                check=False,
                timeout=deadline,
            )
        except subprocess.TimeoutExpired as exc:
            raise MacVmError(f"boot-macos timed out after {deadline}s") from exc
        shots = sorted(pathlib.Path(tmp).glob("shot.*.png"))
        if not shots:
            raise MacVmError(
                f"boot-macos produced no screenshot (rc={result.returncode}): {result.stderr.strip()}"
            )
        # Load and detach from the temp file before it is removed.
        with Image.open(shots[-1]) as img:
            return img.convert("RGB")


def screenshot_many(
    bundles: Sequence[str | os.PathLike],
    seconds: int = 20,
    timeout: float | None = None,
    shares: Sequence[str] | None = None,
    max_workers: int | None = None,
) -> dict[str, "Image.Image"]:
    """Boot several guests off-screen *concurrently* and return one frame each.

    Each bundle runs in its own ``macos-vm`` process, so the boots are fully
    independent and fan out across a thread pool. Returns a dict keyed by the
    string form of each input path to its last-frame ``PIL.Image``. ``shares``
    (if given) is applied to every guest.

    Raises :class:`MacVmError` (or the underlying error) if any guest fails; the
    first failure encountered is re-raised after every process has been
    resolved, so none is left orphaned. Tune parallelism with ``max_workers``
    (defaults to one worker per bundle).
    """
    from concurrent.futures import ThreadPoolExecutor

    keys = [str(b) for b in bundles]
    if not keys:
        return {}
    workers = max_workers if max_workers is not None else len(keys)
    results: dict[str, Image.Image] = {}
    error: BaseException | None = None
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {
            key: pool.submit(screenshot, bundle, seconds, timeout, shares)
            for key, bundle in zip(keys, bundles)
        }
        # Resolve every future (each owns a process) before re-raising, so a
        # failure in one guest does not leak the others.
        for key, future in futures.items():
            try:
                results[key] = future.result()
            except BaseException as exc:  # noqa: BLE001 - first failure re-raised below
                if error is None:
                    error = exc
    if error is not None:
        raise error
    return results


class Driver:
    """Drive a booted macOS guest in lockstep over the binary's ``drive-macos``
    mode.

    Spawns one ``macos-vm`` process that boots the guest off-screen and reads
    newline commands from stdin, acking each on stdout. Use it as a context
    manager so the guest is always stopped on exit::

        with macvm.Driver("/path/to/guest", shares=["auto=/host/app"]) as d:
            d.click(0.5, 0.5)
            d.type_("ls")
            d.key("return")
            img = d.shot()

    Every method returns the binary's one-line acknowledgement; :meth:`shot`
    returns a ``PIL.Image`` instead. An ``err ...`` ack, or the process dying,
    raises :class:`MacVmError`. Each :class:`Driver` is its own process, so
    independent instances drive different guests in parallel.
    """

    def __init__(
        self,
        bundle: str | os.PathLike,
        shares: Sequence[str] | None = None,
        timeout: float = 120,
    ) -> None:
        """Prepare a driver for ``bundle`` (the guest boots on :meth:`__enter__`).

        ``shares`` is a list of ``"TAG=HOSTDIR"`` virtio-fs specs (see the
        module docstring). ``timeout`` bounds how long :meth:`close` waits for
        the process to quit; per-command reads block until the ack arrives, since
        a slow guest boot can delay the first one.
        """
        self._bundle = str(bundle)
        self._shares = list(shares) if shares else []
        self.timeout = timeout
        self._proc: subprocess.Popen[str] | None = None

    def __enter__(self) -> "Driver":
        bin_path = _binary()
        # stderr carries only the binary's boot/log lines; stdout carries the
        # acks, one per command. Send stderr to DEVNULL so a clean stdout read
        # never has to skip non-ack noise. The signed re-exec inherits these
        # pipes, so the channel survives it.
        self._proc = subprocess.Popen(
            [bin_path, "drive-macos", "--bundle", self._bundle, *_share_args(self._shares)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def close(self) -> None:
        """Quit the guest and tear down the process. Idempotent."""
        proc = self._proc
        self._proc = None
        if proc is None:
            return
        try:
            # `quit` exits the process with no ack, so write it directly rather
            # than through `send` (which would wait for an ack that never comes).
            if proc.poll() is None and proc.stdin is not None:
                try:
                    proc.stdin.write("quit\n")
                    proc.stdin.flush()
                except (BrokenPipeError, OSError):
                    pass
            try:
                proc.wait(timeout=self.timeout)
            except subprocess.TimeoutExpired:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
        finally:
            for stream in (proc.stdin, proc.stdout):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError:
                        pass

    def send(self, command: str) -> str:
        """Write one ``command`` line, flush, and return its one-line ack.

        Raises :class:`MacVmError` on an ``err ...`` ack, or if the driver
        process has died or closed its output.
        """
        proc = self._proc
        if proc is None or proc.stdin is None or proc.stdout is None:
            raise MacVmError("driver is not running (use it as a context manager)")
        if proc.poll() is not None:
            raise MacVmError(f"driver process exited with code {proc.returncode}")
        line = command.rstrip("\n")
        try:
            proc.stdin.write(line + "\n")
            proc.stdin.flush()
        except (BrokenPipeError, OSError) as exc:
            raise MacVmError(f"driver process closed its input: {exc}") from exc
        # stderr is discarded, so the next stdout line is this command's ack;
        # skip a stray blank line all the same.
        while True:
            ack = proc.stdout.readline()
            if ack == "":
                rc = proc.poll()
                raise MacVmError(
                    f"driver process gave no ack for {command!r} "
                    f"(process exited with code {rc})"
                )
            ack = ack.rstrip("\n")
            if ack != "":
                break
        if ack.startswith("err"):
            raise MacVmError(f"command {command!r} failed: {ack}")
        return ack

    def key(self, name: str, count: int = 1) -> str:
        """Press a named key (``return``, ``tab``, arrows, ``f1``..``f12``, a
        modifier) ``count`` times."""
        return self.send(f"key {name} {count}")

    def press_down(self, name: str) -> str:
        """Hold a key down (e.g. a modifier, to chord it with :meth:`key`)."""
        return self.send(f"down {name}")

    def release(self, name: str) -> str:
        """Release a key held with :meth:`press_down`."""
        return self.send(f"up {name}")

    def type_(self, text: str) -> str:
        """Type ``text`` as printable ASCII characters (US layout).

        A newline cannot be sent in ``text`` (it would split the stdin command);
        press ``return`` with :meth:`key` instead.
        """
        return self.send(f"type {text}")

    def click(self, fx: float, fy: float) -> str:
        """Left-click at fraction ``(fx, fy)`` of the display, from the top-left
        (resolution-independent, both in ``0..1``)."""
        return self.send(f"click {fx} {fy}")

    def wait(self, seconds: float) -> str:
        """Sleep ``seconds`` in the guest driver (fractional allowed)."""
        return self.send(f"wait {seconds}")

    def shot(self, path: str | os.PathLike | None = None) -> "Image.Image":
        """Screenshot the guest framebuffer and return a ``PIL.Image``.

        With ``path``, the PNG is also written there. With no ``path``, it goes
        to a temp file that is loaded and removed. Raises :class:`MacVmError` if
        the capture fails.
        """
        from PIL import Image

        if path is not None:
            out = pathlib.Path(path)
            self.send(f"shot {out}")
            with Image.open(out) as img:
                return img.convert("RGB")
        with tempfile.TemporaryDirectory(prefix="ix-macvm-shot-") as tmp:
            out = pathlib.Path(tmp) / "shot.png"
            self.send(f"shot {out}")
            # Load and detach before the temp dir is removed.
            with Image.open(out) as img:
                return img.convert("RGB")


def drive(
    bundle: str | os.PathLike,
    commands: Sequence[str],
    shares: Sequence[str] | None = None,
    timeout: float = 120,
) -> list[str]:
    """Open a :class:`Driver` for ``bundle``, send each command, return the acks.

    A one-shot convenience for a fixed script: it boots the guest, runs
    ``commands`` in order, and stops the guest on the way out. ``shares`` and
    ``timeout`` are as on :class:`Driver`. Use :class:`Driver` directly when the
    next command depends on a captured frame. Raises :class:`MacVmError` on any
    failing command.
    """
    with Driver(bundle, shares=shares, timeout=timeout) as d:
        return [d.send(command) for command in commands]
