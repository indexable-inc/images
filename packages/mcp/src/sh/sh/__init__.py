"""Run a shell command on the kernel's async loop and render it two ways.

Bundled like ``view``/``fff``/``fleet`` so every session can ``import sh`` with no
setup. The point: when you genuinely need to shell out (a ``gh``/``git``/``nix``
invocation with no Python binding), do it without blocking the one shared event
loop and without leaking terminal escape codes into your own context.

    import sh
    out = await sh("gh run list --limit 5")
    out                       # last expr: dashboard shows the COLORED terminal
                              # block, you get the escape-stripped plain text

``sh`` is async (built on :func:`asyncio.create_subprocess_shell`), so it never
freezes the kernel the way a bare ``subprocess.run`` does. The value it returns is
an :class:`Output`, which is a ``Result`` subclass: ending a cell with it
satisfies the kernel's Result contract directly, the human watching the dashboard
sees the command's real ANSI color rendered to HTML, and the model's tool result
gets the same output with every escape sequence stripped. The two never cross.

Color is captured by telling the child it may emit it (``FORCE_COLOR=1`` /
``CLICOLOR_FORCE=1``) while still capturing through pipes, so modern tools
(``gh``, ``git``, ``cargo``, ``rg``, ``eza``) produce clean SGR color with none of
the cursor-movement noise a PTY would inject. Pass ``color=False`` to disable it.

The :class:`Output` also exposes the parts programmatically::

    out.code     # exit status (int)
    out.ok       # out.code == 0
    out.text     # combined stdout+stderr, escape codes stripped
    out.raw      # the same, with the original ANSI color preserved
    out.cmd      # the command that was run

An ``Output`` also behaves like its text for the common string operations
(``out[-4000:]``, ``out + "..."``, ``"error" in out``, ``len(out)``,
``str(out)``), so composing command output needs no ``str(...)`` wrapping.

stdout and stderr are merged in emission order (terminal-style). A non-zero exit
is surfaced, never swallowed: the model view appends an ``[exit N]`` marker, and
``await sh(cmd, check=True)`` raises :class:`ShellError` instead of returning.

Inside the kernel the child's output also streams to the running cell's stdout
as it arrives, so it lands in ``jobs['<id>'].output`` live: a long command's log
is pageable from the job even when the cell backgrounds (or is cancelled) before
the ``Output`` value is ever bound. Cancelling the task kills the child's whole
process group, never orphaning it.
"""

from __future__ import annotations

import asyncio
import codecs
import html as _html
import os
import re
import shlex
import signal
import sys

__all__ = ["sh", "Output", "ShellError"]

__version__ = "0.1.0"

# `Result` is the kernel runtime's human/model split. Importing it lets an
# `Output` BE a Result, so a cell can end with `await sh(...)` and satisfy the
# contract with no `Result.of(...)` wrapper. Outside the kernel (plain `import
# sh` in a script or a test) the runtime is absent; fall back to `object` so the
# module still imports and `_repr_html_`/`__repr__` carry the rendering.
try:
    from ix_notebook_mcp.runtime import Result as _ResultBase
    from ix_notebook_mcp.runtime import _ANSI, _ansi_to_html, _ix_current, _strip_ansi

    _HAS_RESULT = True
except Exception:  # pragma: no cover - exercised only outside the kernel
    # Standalone (`import sh` with no kernel): degrade gracefully. The canonical
    # ANSI handling lives in the runtime; without it, strip nothing and merely
    # escape for HTML rather than reimplement the escape grammar here.
    _ResultBase = object
    _HAS_RESULT = False
    _ix_current = None
    # SGR color only; the full escape grammar is the runtime's to own.
    _ANSI = re.compile(r"\x1b\[[0-9;]*m")

    def _strip_ansi(text: str) -> str:
        return _ANSI.sub("", text)

    def _ansi_to_html(text: str) -> str:
        return _html.escape(text)

# Environment that asks well-behaved CLIs to emit SGR color even though their
# stdout is a pipe, not a TTY. PAGER=cat keeps a tool that auto-pages (git, gh)
# from blocking forever on a captured stream.
_COLOR_ENV = {
    "FORCE_COLOR": "1",
    "CLICOLOR_FORCE": "1",
    "CLICOLOR": "1",
    "TERM": "xterm-256color",
    "GIT_PAGER": "cat",
    "PAGER": "cat",
}

_MONO = "ui-monospace,SFMono-Regular,Menlo,monospace"


class ShellError(RuntimeError):
    """Raised by ``await sh(cmd, check=True)`` when the command exits non-zero.

    Carries the :class:`Output` so the failing command's text is still
    inspectable: ``except ShellError as e: print(e.output.text)``.
    """

    def __init__(self, output: "Output") -> None:
        self.output = output
        super().__init__(f"command exited {output.code}: {output.cmd}")


class Output(_ResultBase):
    """The result of one :func:`sh` call: a colored view for the human, escape-
    stripped text for the model.

    It is a ``Result`` subclass, so returning it as a cell's final expression
    renders ``user_html`` (the ANSI-to-HTML terminal block) on the dashboard and
    hands the model ``llm_result`` (the same output with escape codes removed).
    """

    def __init__(self, *, cmd: str, code: int, raw: str, duration: float) -> None:
        self.cmd = cmd
        self.code = code
        self.raw = raw
        self.duration = duration
        if _HAS_RESULT:
            super().__init__(
                user_html=self._render_html(),
                llm_result=self._render_text(),
                llm_images=[],
            )

    @property
    def ok(self) -> bool:
        return self.code == 0

    @property
    def text(self) -> str:
        """Combined stdout+stderr with ANSI escape codes stripped."""
        return _strip_ansi(self.raw)

    def lines(self) -> list[str]:
        """The escape-stripped output split into lines (trailing newline dropped)."""
        return self.text.splitlines()

    def _render_text(self) -> str:
        body = self.text
        if self.code == 0:
            return body
        # Flag a failure so the model never reads non-zero output as success.
        marker = f"[exit {self.code}]"
        return f"{body}\n{marker}" if body else marker

    def _render_html(self) -> str:
        body = _ansi_to_html(self.raw)
        badge_color = "#7bd88f" if self.code == 0 else "#fc618d"
        badge = (
            f'<span style="color:{badge_color}">exit {self.code}</span>'
            f'<span style="color:#6a6a70"> · {self.duration:.2f}s</span>'
        )
        prompt = (
            f'<div style="color:#6a6a70;padding:6px 10px 0">'
            f'<span style="color:#7bd88f">$</span> '
            f'{_html.escape(self.cmd)}</div>'
        )
        out = (
            f'<pre style="margin:0;padding:6px 10px 10px;white-space:pre-wrap;'
            f'word-break:break-word">{body}</pre>'
        )
        foot = f'<div style="padding:0 10px 6px;font-size:11px">{badge}</div>'
        return (
            f'<div style="background:#141416;border:1px solid #242427;border-radius:6px;'
            f'color:#e6e6e6;font-family:{_MONO};font-size:12px;overflow:auto">'
            f"{prompt}{out}{foot}</div>"
        )

    def __repr__(self) -> str:
        return self._render_text()

    def _repr_html_(self) -> str:
        return self._render_html()

    # An Output composes like its text: slice it, concatenate it, search it,
    # measure it -- no `str(...)` wrapping. All delegate to `.text` (the
    # escape-stripped output), the same view `str(out)` returns.
    def __str__(self) -> str:
        return self.text

    def __bool__(self) -> bool:
        # Defining __len__ would otherwise make an empty (but successful) output
        # falsy; an Output is a result object, so it is always truthy -- test
        # success with `.ok`, emptiness with `len(out)`.
        return True

    def __getitem__(self, key) -> str:
        return self.text[key]

    def __len__(self) -> int:
        return len(self.text)

    def __contains__(self, item) -> bool:
        return item in self.text

    def __add__(self, other) -> str:
        return self.text + other

    def __radd__(self, other) -> str:
        return other + self.text


def _terminate(proc: asyncio.subprocess.Process) -> None:
    """Kill the child and the process group it leads.

    ``sh`` starts each child in its own session (``start_new_session=True``), so a
    command that backgrounds a grandchild (which would otherwise keep the merged
    stdout pipe open and hang the reap forever) is killed as a group here.
    """
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        # Process already gone, or no group to signal: kill the child directly.
        try:
            proc.kill()
        except ProcessLookupError:
            pass


class _EchoStripper:
    """Incrementally strip ANSI escapes from streamed chunks.

    A chunk boundary can split an escape sequence in two; a naive per-chunk
    ``_strip_ansi`` would then leak half of it as visible garbage. This holds
    back a trailing, still-incomplete escape and prepends it to the next chunk,
    so the echoed stream is clean no matter where the pipe chops it.
    """

    def __init__(self) -> None:
        self._pending = ""

    def feed(self, text: str) -> str:
        text = self._pending + text
        self._pending = ""
        cut = text.rfind("\x1b")
        if cut != -1:
            tail = text[cut:]
            # A complete sequence (or ESC followed by plain text) strips fine;
            # only a short, genuinely unfinished introducer is held back.
            if _ANSI.match(tail) is None and len(tail) < 64:
                self._pending = tail
                text = text[:cut]
        return _strip_ansi(text)

    def flush(self) -> str:
        text, self._pending = self._pending, ""
        return _strip_ansi(text)


def _in_kernel_job() -> bool:
    """True when this call runs inside a kernel job, where ``sys.stdout`` routes
    to that job's captured output (the runtime's tee)."""
    return _ix_current is not None and _ix_current.get() is not None


async def sh(
    cmd: str | list[str],
    *,
    cwd: str | os.PathLike | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    check: bool = False,
    color: bool = True,
    echo: bool | None = None,
) -> Output:
    """Run ``cmd`` on the shared async loop and return its :class:`Output`.

    ``cmd`` is a string (run through the shell, so pipes and globs work) or an
    argv list (executed directly, no shell parsing). stdout and stderr are merged
    in order. ``cwd`` is the directory to run in (defaults to the kernel's
    current directory); pass it instead of a `cd X && ...` prefix, which is
    rejected, so the command string stays clean. ``env`` extends the environment;
    ``timeout`` (seconds) kills the child's whole process group and raises
    :class:`TimeoutError`; ``check=True`` raises :class:`ShellError` on a non-zero
    exit; ``color=False`` suppresses the forced-color environment.

    Output STREAMS as it arrives: inside the kernel each chunk is echoed
    (escape-stripped) to the running cell's stdout, so a long command's log is in
    ``jobs['<id>'].output`` live and survives the cell backgrounding or being
    cancelled. ``echo`` overrides that default (it is off outside the kernel).
    Cancelling the awaiting task kills the child's whole process group, so a
    cancelled cell never leaves an orphan running (or holding a lock) behind.

    With no ``timeout`` a command that keeps the stdout pipe open (a daemon it
    backgrounds, say) waits for that pipe to close. The await yields to the loop,
    so it never blocks other jobs; pass ``timeout`` to bound such a command.
    """
    if isinstance(cmd, str) and re.match(r"\s*cd\b", cmd):
        raise ValueError(
            "sh() takes no `cd ...` prefix: pass the working directory as cwd= and keep "
            "the command itself clean, e.g. await sh('ix trace <id>', cwd='/path/to/repo')."
        )
    full_env = dict(os.environ)
    if color:
        full_env.update(_COLOR_ENV)
    if env:
        full_env.update(env)

    if isinstance(cmd, (list, tuple)):
        argv = [str(part) for part in cmd]
        shown = shlex.join(argv)
        proc = await asyncio.create_subprocess_exec(
            *argv,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            cwd=cwd,
            env=full_env,
            start_new_session=True,
        )
    else:
        shown = cmd
        proc = await asyncio.create_subprocess_shell(
            cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            cwd=cwd,
            env=full_env,
            start_new_session=True,
        )

    do_echo = _in_kernel_job() if echo is None else echo
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    stripper = _EchoStripper()
    chunks: list[str] = []

    def _keep(text: str) -> None:
        chunks.append(text)
        if do_echo:
            sys.stdout.write(stripper.feed(text))

    async def _drain() -> None:
        while True:
            block = await proc.stdout.read(8192)
            if not block:
                break
            _keep(decoder.decode(block))
        tail = decoder.decode(b"", final=True)
        if tail:
            _keep(tail)
        if do_echo:
            sys.stdout.write(stripper.flush())
        await proc.wait()

    loop = asyncio.get_running_loop()
    started = loop.time()
    try:
        if timeout is not None:
            await asyncio.wait_for(_drain(), timeout)
        else:
            await _drain()
    except asyncio.TimeoutError:
        _terminate(proc)
        # The group is dead, so the pipe closes and this reap returns promptly;
        # bound it anyway so a wedged reap can never hang the job past its timeout.
        try:
            await asyncio.wait_for(proc.wait(), 2.0)
        except asyncio.TimeoutError:
            pass
        raise TimeoutError(f"command timed out after {timeout}s: {shown}") from None
    except asyncio.CancelledError:
        # The awaiting task was cancelled (jobs['<id>'].cancel()): take the child
        # and its whole group down with it, so a cancelled cell never leaves an
        # orphan still running (and holding locks) in the background.
        _terminate(proc)
        raise

    duration = loop.time() - started
    out = Output(
        cmd=shown,
        code=proc.returncode if proc.returncode is not None else -1,
        raw="".join(chunks),
        duration=duration,
    )
    if check and not out.ok:
        raise ShellError(out)
    return out


# Make the module itself callable, so the documented `import sh; await sh(cmd)`
# works without reaching for `sh.sh`. The module object's class is swapped for a
# ModuleType subclass that forwards a call to the sh() coroutine function. The
# kernel binds this same module object as `sh` in the user namespace too (see
# ix_notebook_mcp.runtime.install), so `await sh(...)` works with or without an
# explicit import, while `sh.Output` / `sh.ShellError` stay reachable as attrs.
import types as _types


class _CallableModule(_types.ModuleType):
    def __call__(self, *args, **kwargs):
        return sh(*args, **kwargs)


sys.modules[__name__].__class__ = _CallableModule
